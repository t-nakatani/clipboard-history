use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use clipboard_core::{
    ClipCandidate, ClipId, HistoryCursor, HistoryPage, HistoryRepository, ImagePreview,
    PageDirection, PlannedQuery, Representation, UpsertOutcome,
};
use rusqlite::Connection;

use crate::{
    GarbageCollectionStats, PayloadStore, StoreError, configure_connection, gc, migrate,
    recovery::{
        StartupRecoveryReport, is_corrupt_database_error, mark_store_running,
        perform_startup_recovery, quarantine_store, remove_running_marker,
        resume_quarantine_if_needed, running_marker_path,
    },
    repository,
};

#[derive(Clone, Debug)]
pub struct StoreOptions {
    pub database_path: PathBuf,
    pub payload_directory: PathBuf,
    pub cache_kib: usize,
    pub inline_threshold: usize,
    pub max_history_items: usize,
    pub prune_batch_size: usize,
    pub max_restore_bytes: usize,
    pub maintenance: MaintenancePolicy,
}

impl StoreOptions {
    pub fn new(database_path: impl Into<PathBuf>, payload_directory: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            payload_directory: payload_directory.into(),
            cache_kib: 1024,
            inline_threshold: 16 * 1024,
            max_history_items: 100_000,
            prune_batch_size: 1_000,
            max_restore_bytes: 64 * 1024 * 1024,
            maintenance: MaintenancePolicy::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreStats {
    pub page_count: u64,
    pub freelist_count: u64,
    pub wal_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointResult {
    pub busy: u64,
    pub log_frames: u64,
    pub checkpointed_frames: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceTrigger {
    Periodic,
    Idle,
    DeepIdle,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaintenanceReport {
    pub garbage_collection: GarbageCollectionStats,
    pub passive_checkpoint: Option<CheckpointResult>,
    pub truncate_checkpoint: Option<CheckpointResult>,
    pub vacuum_pages: u64,
    pub fts_optimized: bool,
}

#[derive(Clone, Debug)]
pub struct MaintenancePolicy {
    pub passive_checkpoint_wal_bytes: u64,
    pub truncate_checkpoint_wal_bytes: u64,
    pub vacuum_freelist_pages: u64,
    pub vacuum_step_pages: u64,
    pub gc_batch_size: usize,
    pub fts_optimize_deleted_rows: u64,
}

impl Default for MaintenancePolicy {
    fn default() -> Self {
        Self {
            passive_checkpoint_wal_bytes: 4 * 1024 * 1024,
            truncate_checkpoint_wal_bytes: 16 * 1024 * 1024,
            vacuum_freelist_pages: 2_048,
            vacuum_step_pages: 256,
            gc_batch_size: 256,
            fts_optimize_deleted_rows: 10_000,
        }
    }
}

enum Command {
    Upsert(ClipCandidate, Sender<Result<UpsertOutcome, StoreError>>),
    RecentPage(
        Option<HistoryCursor>,
        PageDirection,
        usize,
        Sender<Result<HistoryPage, StoreError>>,
    ),
    Representations(ClipId, Sender<Result<Vec<Representation>, StoreError>>),
    ImagePreview(ClipId, Sender<Result<Option<ImagePreview>, StoreError>>),
    SearchPage(
        PlannedQuery,
        Option<HistoryCursor>,
        PageDirection,
        usize,
        Sender<Result<HistoryPage, StoreError>>,
    ),
    QuickCheck(Sender<Result<(), StoreError>>),
    Stats(Sender<Result<StoreStats, StoreError>>),
    CheckpointPassive(Sender<Result<CheckpointResult, StoreError>>),
    CheckpointTruncate(Sender<Result<CheckpointResult, StoreError>>),
    IncrementalVacuum(u64, Sender<Result<(), StoreError>>),
    Delete(ClipId, Sender<Result<bool, StoreError>>),
    CollectGarbage(usize, Sender<Result<GarbageCollectionStats, StoreError>>),
    RecoverOrphans(Sender<Result<GarbageCollectionStats, StoreError>>),
    RecoverStartup(Sender<Result<StartupRecoveryReport, StoreError>>),
    RunMaintenance(
        MaintenanceTrigger,
        Sender<Result<MaintenanceReport, StoreError>>,
    ),
    Shutdown(Sender<Result<(), StoreError>>),
}

pub struct StoreHandle {
    sender: Sender<Command>,
    actor: Option<thread::JoinHandle<()>>,
    startup_recovery_required: bool,
}

impl StoreHandle {
    pub fn open(options: StoreOptions) -> Result<Self, StoreError> {
        if options.max_history_items == 0 || options.prune_batch_size == 0 {
            return Err(StoreError::InvalidData(
                "retention limits must be greater than zero",
            ));
        }
        if let Some(parent) = options.database_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let marker_path = running_marker_path(&options.database_path);
        let startup_recovery_required = mark_store_running(&marker_path)?;
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let marker_created_for_this_open = !startup_recovery_required;
        let cleanup_marker_path = marker_path.clone();
        let actor = thread::Builder::new()
            .name("clipboard-store".into())
            .spawn(move || {
                run_actor(
                    options,
                    receiver,
                    ready_sender,
                    marker_path,
                    startup_recovery_required,
                    marker_created_for_this_open,
                )
            });
        let actor = match actor {
            Ok(actor) => actor,
            Err(error) => {
                if marker_created_for_this_open {
                    let _ = fs::remove_file(cleanup_marker_path);
                }
                return Err(error.into());
            }
        };
        if let Err(error) = ready_receiver
            .recv()
            .map_err(|_| StoreError::ActorStopped)?
        {
            let _ = actor.join();
            return Err(error);
        }
        Ok(Self {
            sender,
            actor: Some(actor),
            startup_recovery_required,
        })
    }

    pub const fn startup_recovery_required(&self) -> bool {
        self.startup_recovery_required
    }

    pub fn recover_startup(&self) -> Result<StartupRecoveryReport, StoreError> {
        self.request(Command::RecoverStartup)
    }

    pub fn shutdown(&self) -> Result<(), StoreError> {
        self.request(Command::Shutdown)
    }

    pub fn quick_check(&self) -> Result<(), StoreError> {
        self.request(Command::QuickCheck)
    }

    pub fn stats(&self) -> Result<StoreStats, StoreError> {
        self.request(Command::Stats)
    }

    pub fn checkpoint_passive(&self) -> Result<CheckpointResult, StoreError> {
        self.request(Command::CheckpointPassive)
    }

    pub fn checkpoint_truncate(&self) -> Result<CheckpointResult, StoreError> {
        self.request(Command::CheckpointTruncate)
    }

    pub fn incremental_vacuum(&self, pages: u64) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::IncrementalVacuum(pages, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn delete(&self, id: ClipId) -> Result<bool, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::Delete(id, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn collect_garbage(&self, limit: usize) -> Result<GarbageCollectionStats, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::CollectGarbage(limit, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn recover_orphans(&self) -> Result<GarbageCollectionStats, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::RecoverOrphans(reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn run_maintenance(
        &self,
        trigger: MaintenanceTrigger,
    ) -> Result<MaintenanceReport, StoreError> {
        self.request(|reply| Command::RunMaintenance(trigger, reply))
    }

    fn request<T>(
        &self,
        make_command: impl FnOnce(Sender<Result<T, StoreError>>) -> Command,
    ) -> Result<T, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(make_command(reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }
}

impl HistoryRepository for StoreHandle {
    type Error = StoreError;

    fn upsert(&self, candidate: ClipCandidate) -> Result<UpsertOutcome, Self::Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::Upsert(candidate, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    fn recent_page(
        &self,
        cursor: Option<HistoryCursor>,
        direction: PageDirection,
        limit: usize,
    ) -> Result<HistoryPage, Self::Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::RecentPage(cursor, direction, limit, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    fn representations(&self, id: ClipId) -> Result<Vec<Representation>, Self::Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::Representations(id, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    fn image_preview(&self, id: ClipId) -> Result<Option<ImagePreview>, Self::Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ImagePreview(id, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    fn search_page(
        &self,
        query: PlannedQuery,
        cursor: Option<HistoryCursor>,
        direction: PageDirection,
        limit: usize,
    ) -> Result<HistoryPage, Self::Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::SearchPage(query, cursor, direction, limit, reply))
            .map_err(|_| StoreError::ActorStopped)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }
}

impl Drop for StoreHandle {
    fn drop(&mut self) {
        let (reply, receiver) = mpsc::channel();
        let _ = self.sender.send(Command::Shutdown(reply));
        let _ = receiver.recv();
        if let Some(actor) = self.actor.take() {
            let _ = actor.join();
        }
    }
}

fn run_actor(
    options: StoreOptions,
    receiver: Receiver<Command>,
    ready: Sender<Result<(), StoreError>>,
    marker_path: PathBuf,
    startup_recovery_required: bool,
    marker_created_for_this_open: bool,
) {
    let resumed_quarantine = if startup_recovery_required {
        resume_quarantine_if_needed(&options)
    } else {
        Ok(None)
    };
    let mut initial_quarantine = match resumed_quarantine {
        Ok(quarantine) => quarantine,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let result = open_initialized_store(&options).or_else(|error| {
        if startup_recovery_required
            && initial_quarantine.is_none()
            && is_corrupt_database_error(&error)
        {
            let quarantine = quarantine_store(&options)?;
            initial_quarantine = Some(quarantine);
            open_initialized_store(&options)
        } else {
            Err(error)
        }
    });
    let Ok((mut connection, mut payload_store, mut live_count)) = result else {
        if marker_created_for_this_open {
            let _ = fs::remove_file(&marker_path);
        }
        let _ = ready.send(result.map(|_| ()));
        return;
    };
    let mut recovery_pending = startup_recovery_required;
    let _ = ready.send(Ok(()));

    while let Ok(command) = receiver.recv() {
        match command {
            Command::Upsert(candidate, reply) => {
                let result = repository::upsert(
                    &mut connection,
                    &payload_store,
                    options.inline_threshold,
                    candidate,
                    live_count
                        .saturating_add(1)
                        .saturating_sub(options.max_history_items)
                        .min(options.prune_batch_size.max(1)),
                );
                match result {
                    Ok(result) => {
                        let pruned = result.pruned;
                        if result.outcome.inserted {
                            live_count = live_count.saturating_add(1);
                        }
                        live_count = live_count.saturating_sub(pruned);
                        let _ = reply.send(Ok(result.outcome));
                        if pruned != 0 {
                            // The capture reply is sent before physical payload cleanup.
                            // A failed collection leaves the durable queue for a later retry.
                            let _ = gc::collect_queued(
                                &mut connection,
                                &payload_store,
                                options.prune_batch_size,
                            );
                        }
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::RecentPage(cursor, direction, limit, reply) => {
                // The SELECT owns no transaction beyond this single request.
                let _ = reply.send(repository::recent_page(
                    &connection,
                    cursor,
                    direction,
                    limit,
                ));
            }
            Command::Representations(id, reply) => {
                let result = repository::representations(
                    &connection,
                    &payload_store,
                    id,
                    options.max_restore_bytes,
                );
                let _ = reply.send(result);
            }
            Command::ImagePreview(id, reply) => {
                let _ = reply.send(repository::image_preview(&connection, id));
            }
            Command::SearchPage(query, cursor, direction, limit, reply) => {
                let _ = reply.send(repository::search_page(
                    &connection,
                    query,
                    cursor,
                    direction,
                    limit,
                ));
            }
            Command::QuickCheck(reply) => {
                let _ = reply.send(quick_check(&connection));
            }
            Command::Stats(reply) => {
                let result = store_stats(&connection, &options.database_path);
                let _ = reply.send(result);
            }
            Command::CheckpointPassive(reply) => {
                let result = checkpoint(&connection, "PASSIVE");
                let _ = reply.send(result);
            }
            Command::CheckpointTruncate(reply) => {
                let result = checkpoint(&connection, "TRUNCATE");
                let _ = reply.send(result);
            }
            Command::IncrementalVacuum(pages, reply) => {
                let result = connection
                    .execute_batch(&format!("PRAGMA incremental_vacuum({pages})"))
                    .map_err(Into::into);
                let _ = reply.send(result);
            }
            Command::Delete(id, reply) => {
                let result = connection
                    .execute("DELETE FROM clips WHERE id = ?1", [id.0])
                    .map(|deleted| deleted != 0)
                    .map_err(Into::into);
                if matches!(result, Ok(true)) {
                    live_count = live_count.saturating_sub(1);
                }
                let _ = reply.send(result);
            }
            Command::CollectGarbage(limit, reply) => {
                let result = gc::collect_queued(&mut connection, &payload_store, limit);
                let _ = reply.send(result);
            }
            Command::RecoverOrphans(reply) => {
                let result = gc::collect_orphans(&connection, &payload_store);
                let _ = reply.send(result);
            }
            Command::RecoverStartup(reply) => {
                if !recovery_pending {
                    let _ = reply.send(Ok(StartupRecoveryReport::default()));
                    continue;
                }
                match perform_startup_recovery(
                    &mut connection,
                    &mut payload_store,
                    &options,
                    &mut initial_quarantine,
                ) {
                    Ok((report, rebuilt_live_count)) => {
                        if let Some(count) = rebuilt_live_count {
                            live_count = count;
                        }
                        recovery_pending = false;
                        let _ = reply.send(Ok(report));
                    }
                    Err(failure) => {
                        // recovery_pending stays true so the marker survives shutdown and the
                        // next open retries.
                        let _ = reply.send(Err(failure.error));
                        if !failure.connection_usable {
                            break;
                        }
                    }
                }
            }
            Command::RunMaintenance(trigger, reply) => {
                let result = run_maintenance(
                    &mut connection,
                    &payload_store,
                    &options.database_path,
                    &options.maintenance,
                    trigger,
                );
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
                let result = checkpoint(&connection, "TRUNCATE")
                    .map(|_| ())
                    .and_then(|()| {
                        if recovery_pending {
                            Ok(())
                        } else {
                            remove_running_marker(&marker_path)
                        }
                    });
                let _ = reply.send(result);
                break;
            }
        }
    }
}

pub(crate) fn open_store(options: &StoreOptions) -> Result<(Connection, PayloadStore), StoreError> {
    if let Some(parent) = options.database_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = Connection::open(&options.database_path)?;
    configure_connection(&connection, options.cache_kib)?;
    migrate(&mut connection)?;
    Ok((connection, PayloadStore::new(&options.payload_directory)))
}

pub(crate) fn open_initialized_store(
    options: &StoreOptions,
) -> Result<(Connection, PayloadStore, usize), StoreError> {
    let (mut connection, payload_store) = open_store(options)?;
    let mut live_count = repository::count(&connection)?;
    while live_count > options.max_history_items {
        let requested =
            (live_count - options.max_history_items).min(options.prune_batch_size.max(1));
        match repository::prune_oldest(&mut connection, requested)? {
            0 => break,
            deleted => live_count -= deleted,
        }
    }
    Ok((connection, payload_store, live_count))
}

pub(crate) fn quick_check(connection: &Connection) -> Result<(), StoreError> {
    connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(StoreError::from)
        .and_then(|value| {
            (value == "ok")
                .then_some(())
                .ok_or(StoreError::InvalidData("quick_check failed"))
        })
}

fn store_stats(connection: &Connection, path: &std::path::Path) -> Result<StoreStats, StoreError> {
    let page_count =
        connection.pragma_query_value(None, "page_count", |row| row.get::<_, i64>(0))? as u64;
    let freelist_count =
        connection.pragma_query_value(None, "freelist_count", |row| row.get::<_, i64>(0))? as u64;
    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    let wal_bytes = std::fs::metadata(wal_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(StoreStats {
        page_count,
        freelist_count,
        wal_bytes,
    })
}

fn checkpoint(connection: &Connection, mode: &str) -> Result<CheckpointResult, StoreError> {
    connection
        .query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |row| {
            Ok(CheckpointResult {
                busy: row.get::<_, i64>(0)? as u64,
                log_frames: row.get::<_, i64>(1)? as u64,
                checkpointed_frames: row.get::<_, i64>(2)? as u64,
            })
        })
        .map_err(Into::into)
}

fn run_maintenance(
    connection: &mut Connection,
    payload_store: &PayloadStore,
    database_path: &std::path::Path,
    policy: &MaintenancePolicy,
    trigger: MaintenanceTrigger,
) -> Result<MaintenanceReport, StoreError> {
    let garbage_collection = gc::collect_queued(connection, payload_store, policy.gc_batch_size)?;
    let mut report = MaintenanceReport {
        garbage_collection,
        ..MaintenanceReport::default()
    };

    let state = maintenance_state(connection)?;
    if matches!(trigger, MaintenanceTrigger::DeepIdle)
        && state.deleted_since_fts_optimize >= policy.fts_optimize_deleted_rows
    {
        optimize_fts(connection)?;
        report.fts_optimized = true;
    }

    let mut stats = store_stats(connection, database_path)?;
    if !matches!(trigger, MaintenanceTrigger::Periodic)
        && stats.freelist_count >= policy.vacuum_freelist_pages
    {
        connection.execute_batch(&format!(
            "PRAGMA incremental_vacuum({})",
            policy.vacuum_step_pages
        ))?;
        // The vacuum itself appends WAL frames, so the checkpoint decision below
        // has to see the state left behind by it rather than the earlier reading.
        let after_vacuum = store_stats(connection, database_path)?;
        report.vacuum_pages = stats
            .freelist_count
            .saturating_sub(after_vacuum.freelist_count);
        stats = after_vacuum;
    }

    // A PASSIVE checkpoint moves frames back into the database but never shrinks
    // the WAL file, so once the file passes the gate it stays past it until a
    // TRUNCATE runs. Re-running is cheap because SQLite returns immediately when
    // no frame is pending, and the report records only the runs that moved one.
    if stats.wal_bytes >= policy.truncate_checkpoint_wal_bytes
        && !matches!(trigger, MaintenanceTrigger::Periodic)
    {
        report.truncate_checkpoint = Some(checkpoint(connection, "TRUNCATE")?);
    } else if stats.wal_bytes >= policy.passive_checkpoint_wal_bytes {
        let passive = checkpoint(connection, "PASSIVE")?;
        if passive.checkpointed_frames > 0 {
            report.passive_checkpoint = Some(passive);
        }
    }
    Ok(report)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MaintenanceState {
    deleted_since_fts_optimize: u64,
}

fn maintenance_state(connection: &Connection) -> Result<MaintenanceState, StoreError> {
    connection
        .query_row(
            "SELECT deleted_since_fts_optimize FROM maintenance_state WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| MaintenanceState {
            deleted_since_fts_optimize: value.max(0) as u64,
        })
        .map_err(Into::into)
}

fn optimize_fts(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "INSERT INTO clips_fts(clips_fts) VALUES ('optimize');
         UPDATE maintenance_state
         SET deleted_since_fts_optimize = 0,
             last_fts_optimize_at = unixepoch('subsec') * 1000
         WHERE id = 1;",
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::{
        QuarantinePhase, quarantine_manifest_path, quarantined_database_path,
        quarantined_payload_path, write_quarantine_manifest,
    };
    use clipboard_core::{
        CaptureOutcome, ClipKind, ClipboardSnapshot, HistoryService, ImagePreview, Representation,
        SearchTextPolicy,
    };
    use std::fs::OpenOptions;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{io::Seek, io::SeekFrom, io::Write};

    #[test]
    fn actor_persists_and_touches_without_duplicate() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-store-test-{unique}"));
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        let service = HistoryService::new(
            StoreHandle::open(options).unwrap(),
            SearchTextPolicy::default(),
        );
        let snapshot = ClipboardSnapshot {
            representations: vec![Representation {
                uti: "public.utf8-plain-text".into(),
                bytes: b"hello".to_vec(),
            }],
            image_preview: None,
        };
        service
            .capture(snapshot.clone(), ClipKind::Text, 1)
            .unwrap();
        assert!(!service.repository().recent(10).unwrap()[0].has_image_preview);
        let mut touched_snapshot = snapshot;
        touched_snapshot.image_preview = Some(ImagePreview {
            uti: "public.png".into(),
            bytes: vec![1, 2, 3, 4],
        });
        service
            .capture(touched_snapshot, ClipKind::Text, 2)
            .unwrap();
        let rows = service.repository().recent(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].copy_count, 2);
        assert_eq!(rows[0].last_used_at_ms, 2);
        assert!(rows[0].has_image_preview);
        assert_eq!(
            service.image_preview(rows[0].id).unwrap(),
            Some(ImagePreview {
                uti: "public.png".into(),
                bytes: vec![1, 2, 3, 4],
            })
        );
        service.repository().quick_check().unwrap();
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preview_size_is_bounded_before_sql_commit() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-preview-limit-test-{unique}"));
        let service = HistoryService::new(
            StoreHandle::open(StoreOptions::new(
                root.join("history.sqlite"),
                root.join("payloads"),
            ))
            .unwrap(),
            SearchTextPolicy::default(),
        );
        let result = service.capture(
            ClipboardSnapshot {
                representations: vec![Representation {
                    uti: "public.png".into(),
                    bytes: vec![9; 128],
                }],
                image_preview: Some(ImagePreview {
                    uti: "public.png".into(),
                    bytes: vec![7; 64 * 1024 + 1],
                }),
            },
            ClipKind::Image,
            1,
        );
        assert!(matches!(
            result,
            Err(StoreError::InvalidData(
                "image preview must contain between 1 and 65536 bytes"
            ))
        ));
        assert_eq!(service.repository().recent(10).unwrap().len(), 0);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_enqueues_and_gc_removes_external_payload() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-gc-test-{unique}"));
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        let service = HistoryService::new(
            StoreHandle::open(options).unwrap(),
            SearchTextPolicy::default(),
        );
        let snapshot = ClipboardSnapshot {
            representations: vec![Representation {
                uti: "public.data".into(),
                bytes: vec![7; 32 * 1024],
            }],
            image_preview: None,
        };
        let outcome = service.capture(snapshot, ClipKind::Image, 1).unwrap();
        let clipboard_core::CaptureOutcome::Stored(stored) = outcome else {
            panic!("expected stored clip")
        };
        let selected = service.select(stored.id).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].bytes, vec![7; 32 * 1024]);
        assert!(service.repository().delete(stored.id).unwrap());
        let stats = service.repository().collect_garbage(100).unwrap();
        assert_eq!(stats.queued_scanned, 1);
        assert_eq!(stats.payload_files_deleted, 1);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleted_clip_content_is_not_recoverable_from_the_database_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-secure-delete-test-{unique}"));
        let database_path = root.join("history.sqlite");
        let options = StoreOptions::new(&database_path, root.join("payloads"));
        let service = HistoryService::new(
            StoreHandle::open(options).unwrap(),
            SearchTextPolicy::default(),
        );
        // The FTS5 trigram tokenizer indexes three-character tokens, so scanning
        // only for the whole secret would miss index residue entirely. The secret
        // embeds letter runs whose trigrams ("zqx", "qxj", "xjv", "jvw") are rare
        // enough that a stray match in a small SQLite file is implausible.
        let secret = format!("secret-passphrase-zqxjvw-{unique}-do-not-linger");
        let secret_trigrams = ["zqx", "qxj", "xjv", "jvw"];
        let preview_marker: Vec<u8> = format!("preview-marker-{unique}").into_bytes();
        let external_marker: Vec<u8> = format!("external-marker-{unique}")
            .into_bytes()
            .repeat(2048);

        let outcome = service
            .capture(
                ClipboardSnapshot {
                    representations: vec![
                        Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: secret.as_bytes().to_vec(),
                        },
                        Representation {
                            uti: "public.data".into(),
                            bytes: external_marker.clone(),
                        },
                    ],
                    image_preview: Some(ImagePreview {
                        uti: "public.png".into(),
                        bytes: preview_marker.clone(),
                    }),
                },
                ClipKind::Mixed,
                1,
            )
            .unwrap();
        let clipboard_core::CaptureOutcome::Stored(stored) = outcome else {
            panic!("expected stored clip")
        };
        service.repository().checkpoint_truncate().unwrap();
        assert!(
            file_contains(&database_path, secret.as_bytes()),
            "the test is only meaningful if the clip really reached the file"
        );
        for trigram in secret_trigrams {
            assert!(
                file_contains(&database_path, trigram.as_bytes()),
                "expected the trigram index to hold {trigram} before the delete"
            );
        }

        assert!(service.repository().delete(stored.id).unwrap());
        service.repository().collect_garbage(100).unwrap();
        service.repository().checkpoint_truncate().unwrap();
        service.repository().incremental_vacuum(4096).unwrap();
        service.repository().checkpoint_truncate().unwrap();

        // secure_delete zeroes the freed cells and the FTS5 secure-delete option
        // rewrites the segment blobs, so neither the row text, the trigram tokens
        // nor the preview blob may remain in the live file or WAL.
        let mut needles: Vec<&[u8]> = vec![secret.as_bytes(), preview_marker.as_slice()];
        needles.extend(secret_trigrams.iter().map(|trigram| trigram.as_bytes()));
        for path in [
            database_path.clone(),
            PathBuf::from(format!("{}-wal", database_path.display())),
        ] {
            for needle in &needles {
                assert!(
                    !file_contains(&path, needle),
                    "{} still contains deleted clip content {:?}",
                    path.display(),
                    String::from_utf8_lossy(needle)
                );
            }
        }
        assert!(!file_contains(&database_path, &external_marker[..64]));

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn file_contains(path: &std::path::Path, needle: &[u8]) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        bytes.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn gc_keeps_shared_payload_until_last_reference_is_deleted() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-shared-gc-test-{unique}"));
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        let service = HistoryService::new(
            StoreHandle::open(options).unwrap(),
            SearchTextPolicy::default(),
        );
        for (timestamp, text) in [(1, b"first".as_slice()), (2, b"second".as_slice())] {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![
                            Representation {
                                uti: "public.utf8-plain-text".into(),
                                bytes: text.to_vec(),
                            },
                            Representation {
                                uti: "public.data".into(),
                                bytes: vec![7; 32 * 1024],
                            },
                        ],
                        image_preview: None,
                    },
                    ClipKind::Mixed,
                    timestamp,
                )
                .unwrap();
        }
        let rows = service.repository().recent(10).unwrap();
        service.repository().delete(rows[1].id).unwrap();
        let first_gc = service.repository().collect_garbage(100).unwrap();
        assert_eq!(first_gc.referenced_skipped, 1);
        assert_eq!(first_gc.payload_files_deleted, 0);

        service.repository().delete(rows[0].id).unwrap();
        let final_gc = service.repository().collect_garbage(100).unwrap();
        assert_eq!(final_gc.payload_files_deleted, 1);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_enforces_history_limit_without_loading_history() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-retention-test-{unique}"));
        let mut options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        options.max_history_items = 2;
        let service = HistoryService::new(
            StoreHandle::open(options).unwrap(),
            SearchTextPolicy::default(),
        );

        for (timestamp, value) in [(1, "first"), (2, "second"), (3, "third")] {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: value.as_bytes().to_vec(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    timestamp,
                )
                .unwrap();
        }

        let rows = service.repository().recent(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].preview.as_deref(), Some("third"));
        assert_eq!(rows[1].preview.as_deref(), Some("second"));
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn select_rejects_payload_over_restore_memory_limit() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-restore-limit-test-{unique}"));
        let mut options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        options.max_restore_bytes = 8;
        let service = HistoryService::new(
            StoreHandle::open(options).unwrap(),
            SearchTextPolicy::default(),
        );
        let outcome = service
            .capture(
                ClipboardSnapshot {
                    representations: vec![Representation {
                        uti: "public.utf8-plain-text".into(),
                        bytes: b"more than eight bytes".to_vec(),
                    }],
                    image_preview: None,
                },
                ClipKind::Text,
                1,
            )
            .unwrap();
        let clipboard_core::CaptureOutcome::Stored(stored) = outcome else {
            panic!("expected stored clip")
        };
        assert!(matches!(
            service.select(stored.id),
            Err(StoreError::InvalidData(
                "clip exceeds the configured restore byte limit"
            ))
        ));
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_searches_exact_prefix_and_literal_substring_without_fuzzy_matching() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-search-test-{unique}"));
        let service = HistoryService::new(
            StoreHandle::open(StoreOptions::new(
                root.join("history.sqlite"),
                root.join("payloads"),
            ))
            .unwrap(),
            SearchTextPolicy::default(),
        );
        for (timestamp, value) in [
            (1, "alpha"),
            (2, "alphabet"),
            (3, "x alpha y"),
            (4, "100% real"),
            (5, "a_b"),
        ] {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: value.as_bytes().to_vec(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    timestamp,
                )
                .unwrap();
        }

        let exact = service
            .search("alpha", clipboard_core::MatchMode::Exact, 50)
            .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].preview.as_deref(), Some("alpha"));

        let prefix = service
            .search("alpha", clipboard_core::MatchMode::Prefix, 50)
            .unwrap();
        assert_eq!(prefix.len(), 2);
        let substring = service
            .search("alpha", clipboard_core::MatchMode::Substring, 50)
            .unwrap();
        assert_eq!(substring.len(), 3);

        let literal_percent = service
            .search("100%", clipboard_core::MatchMode::Substring, 50)
            .unwrap();
        assert_eq!(literal_percent.len(), 1);
        assert_eq!(literal_percent[0].preview.as_deref(), Some("100% real"));
        let literal_underscore = service
            .search("a_b", clipboard_core::MatchMode::Exact, 50)
            .unwrap();
        assert_eq!(literal_underscore.len(), 1);
        assert_eq!(literal_underscore[0].preview.as_deref(), Some("a_b"));

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_ignores_case_in_every_match_mode() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-search-case-test-{unique}"));
        let service = HistoryService::new(
            StoreHandle::open(StoreOptions::new(
                root.join("history.sqlite"),
                root.join("payloads"),
            ))
            .unwrap(),
            SearchTextPolicy::default(),
        );
        // A needle longer than 64 characters takes the equality branch of the
        // prefix seek instead of the LIKE one, so both are exercised here.
        let long = "Lorem Ipsum Dolor Sit Amet Consectetur Adipiscing Elit Sed Do Eiusmod Tempor";
        for (timestamp, value) in [
            (1, "Alpha"),
            (2, "ALPHABET"),
            (3, "x aLpHa y"),
            (4, "unrelated"),
            (5, long),
        ] {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: value.as_bytes().to_vec(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    timestamp,
                )
                .unwrap();
        }

        let exact = service
            .search("alpha", clipboard_core::MatchMode::Exact, 50)
            .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].preview.as_deref(), Some("Alpha"));

        let prefix = service
            .search("aLPha", clipboard_core::MatchMode::Prefix, 50)
            .unwrap();
        assert_eq!(prefix.len(), 2);

        let substring = service
            .search("ALPHA", clipboard_core::MatchMode::Substring, 50)
            .unwrap();
        assert_eq!(substring.len(), 3);

        // Needles shorter than three characters are planned as a bounded recent
        // scan rather than an index lookup.
        let short_scan = service
            .search("AL", clipboard_core::MatchMode::Substring, 50)
            .unwrap();
        assert_eq!(short_scan.len(), 3);

        let long_prefix = service
            .search(&long.to_uppercase(), clipboard_core::MatchMode::Prefix, 50)
            .unwrap();
        assert_eq!(long_prefix.len(), 1);

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keyset_pages_remain_ordered_across_equal_timestamps_and_mutations() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-keyset-test-{unique}"));
        let service = HistoryService::new(
            StoreHandle::open(StoreOptions::new(
                root.join("history.sqlite"),
                root.join("payloads"),
            ))
            .unwrap(),
            SearchTextPolicy::default(),
        );
        for (timestamp, value) in [
            (30, "item-6"),
            (30, "item-5"),
            (20, "item-4"),
            (20, "item-3"),
            (10, "item-2"),
            (10, "item-1"),
        ] {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: value.as_bytes().to_vec(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    timestamp,
                )
                .unwrap();
        }

        let original = service.repository().recent(10).unwrap();
        let deleted_id = original[4].id;
        let recopy_id = original.last().unwrap().id;
        let first = service
            .repository()
            .recent_page(None, PageDirection::Older, 2)
            .unwrap();
        assert!(first.has_more);
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.items[0].last_used_at_ms, 30);
        assert_eq!(first.items[1].last_used_at_ms, 30);

        // A newer capture must not shift the continuation as OFFSET would.
        service
            .capture(
                ClipboardSnapshot {
                    representations: vec![Representation {
                        uti: "public.utf8-plain-text".into(),
                        bytes: b"new-head".to_vec(),
                    }],
                    image_preview: None,
                },
                ClipKind::Text,
                100,
            )
            .unwrap();
        service.repository().delete(deleted_id).unwrap();
        let first_cursor = cursor_for(first.items.last().unwrap());
        let second = service
            .repository()
            .recent_page(Some(first_cursor), PageDirection::Older, 2)
            .unwrap();
        let second_cursor = cursor_for(second.items.last().unwrap());
        let third = service
            .repository()
            .recent_page(Some(second_cursor), PageDirection::Older, 2)
            .unwrap();

        let ids: Vec<_> = first
            .items
            .iter()
            .chain(&second.items)
            .chain(&third.items)
            .map(|item| item.id)
            .collect();
        let unique_ids: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids.len(), 5);
        assert_eq!(unique_ids.len(), 5);
        assert!(!third.has_more);
        assert!(!ids.contains(&service.repository().recent(1).unwrap()[0].id));
        assert!(!ids.contains(&deleted_id));

        let newer = service
            .repository()
            .recent_page(third.items.first().map(cursor_for), PageDirection::Newer, 2)
            .unwrap();
        assert_eq!(newer.items, second.items);
        assert!(newer.has_more);
        let newest_loaded = service
            .repository()
            .recent_page(newer.items.first().map(cursor_for), PageDirection::Newer, 2)
            .unwrap();
        assert_eq!(newest_loaded.items, first.items);

        // Recopy moves an old row above the cursor. The app responds by
        // resetting the first page from this capture event.
        service
            .capture(
                ClipboardSnapshot {
                    representations: vec![Representation {
                        uti: "public.utf8-plain-text".into(),
                        bytes: b"item-2".to_vec(),
                    }],
                    image_preview: None,
                },
                ClipKind::Text,
                101,
            )
            .unwrap();
        let reset = service
            .repository()
            .recent_page(None, PageDirection::Older, 2)
            .unwrap();
        assert_eq!(reset.items[0].id, recopy_id);
        assert_eq!(reset.items[0].copy_count, 2);

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_uses_the_same_keyset_cursor_for_every_page() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-search-page-test-{unique}"));
        let service = HistoryService::new(
            StoreHandle::open(StoreOptions::new(
                root.join("history.sqlite"),
                root.join("payloads"),
            ))
            .unwrap(),
            SearchTextPolicy::default(),
        );
        for index in 0..7 {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: format!("alpha-{index}").into_bytes(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    100 - index / 2,
                )
                .unwrap();
        }

        let mut cursor = None;
        let mut results = Vec::new();
        loop {
            let page = service
                .search_page(
                    "alpha-",
                    clipboard_core::MatchMode::Prefix,
                    cursor,
                    PageDirection::Older,
                    3,
                )
                .unwrap();
            let next_cursor = page.continuation_cursor;
            results.extend(page.items);
            if !page.has_more {
                break;
            }
            cursor = next_cursor;
        }
        assert_eq!(results.len(), 7);
        assert_eq!(
            results
                .iter()
                .map(|item| item.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            7
        );
        assert!(
            results
                .windows(2)
                .all(|pair| (pair[0].last_used_at_ms, pair[0].id.0)
                    > (pair[1].last_used_at_ms, pair[1].id.0))
        );

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_recent_scan_continues_across_a_sparse_empty_window() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-recent-scan-page-test-{unique}"));
        let service = HistoryService::new(
            StoreHandle::open(StoreOptions::new(
                root.join("history.sqlite"),
                root.join("payloads"),
            ))
            .unwrap(),
            SearchTextPolicy::default(),
        );
        for index in 0..2_105 {
            let value = if index == 2_104 {
                "x-target".to_owned()
            } else {
                format!("row-{index}")
            };
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: value.into_bytes(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    10_000 - index,
                )
                .unwrap();
        }

        let mut cursor = None;
        let mut count = 0;
        let mut saw_empty_truncated_page = false;
        loop {
            let page = service
                .search_page(
                    "x",
                    clipboard_core::MatchMode::Substring,
                    cursor,
                    PageDirection::Older,
                    50,
                )
                .unwrap();
            if page.items.is_empty() && page.truncated {
                saw_empty_truncated_page = true;
            }
            cursor = page.continuation_cursor;
            count += page.items.len();
            if !page.has_more {
                break;
            }
        }
        assert!(saw_empty_truncated_page);
        assert_eq!(count, 1);

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_shutdown_removes_marker_and_skips_next_recovery() {
        let root = unique_test_root("clipboard-clean-shutdown-test");
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        let marker = running_marker_path(&options.database_path);

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(!store.startup_recovery_required());
        assert!(marker.exists());
        store.shutdown().unwrap();
        assert!(!marker.exists());
        drop(store);

        let reopened = StoreHandle::open(options).unwrap();
        assert!(!reopened.startup_recovery_required());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unclean_marker_enables_recovery_without_blocking_recent_open() {
        let root = unique_test_root("clipboard-unclean-recovery-test");
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        drop(StoreHandle::open(options.clone()).unwrap());

        let orphan = PayloadStore::new(&options.payload_directory)
            .put(b"unreferenced payload")
            .unwrap();
        std::fs::write(
            running_marker_path(&options.database_path),
            b"stale marker\n",
        )
        .unwrap();

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(store.startup_recovery_required());
        assert!(store.recent(10).unwrap().is_empty());
        let report = store.recover_startup().unwrap();
        assert!(report.was_unclean);
        assert!(report.quick_check_passed);
        assert!(!report.database_rebuilt);
        assert_eq!(report.garbage_collection.orphan_files_deleted, 1);
        assert!(
            !PayloadStore::new(&options.payload_directory)
                .path_for(orphan.hash)
                .exists()
        );

        drop(store);
        assert!(!running_marker_path(&options.database_path).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_unclean_database_and_payloads_are_quarantined() {
        let root = unique_test_root("clipboard-corrupt-recovery-test");
        std::fs::create_dir_all(&root).unwrap();
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        std::fs::write(&options.database_path, b"not a sqlite database").unwrap();
        std::fs::create_dir_all(&options.payload_directory).unwrap();
        std::fs::write(
            options.payload_directory.join("keep-for-recovery"),
            b"payload",
        )
        .unwrap();
        std::fs::write(
            running_marker_path(&options.database_path),
            b"stale marker\n",
        )
        .unwrap();

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(store.startup_recovery_required());
        assert!(store.recent(10).unwrap().is_empty());
        let report = store.recover_startup().unwrap();
        assert!(report.database_rebuilt);
        assert!(!report.quick_check_passed);
        assert!(
            report
                .quarantine_path
                .as_ref()
                .is_some_and(|path| path.exists())
        );
        assert!(
            root.read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("payloads.corrupt-")
                })
        );

        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corruption_found_during_initial_count_is_quarantined() {
        let root = unique_test_root("clipboard-post-open-corruption-test");
        let mut options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        let service = HistoryService::new(
            StoreHandle::open(options.clone()).unwrap(),
            SearchTextPolicy::default(),
        );
        for index in 0..200 {
            service
                .capture(
                    ClipboardSnapshot {
                        representations: vec![Representation {
                            uti: "public.utf8-plain-text".into(),
                            bytes: format!("corruption-fixture-{index}").into_bytes(),
                        }],
                        image_preview: None,
                    },
                    ClipKind::Text,
                    index,
                )
                .unwrap();
        }
        drop(service);

        let connection = Connection::open(&options.database_path).unwrap();
        let page_size = connection
            .pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))
            .unwrap() as u64;
        let clips_root_page = connection
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name = 'clips'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap() as u64;
        drop(connection);
        let mut database = OpenOptions::new()
            .write(true)
            .open(&options.database_path)
            .unwrap();
        database
            .seek(SeekFrom::Start((clips_root_page - 1) * page_size))
            .unwrap();
        database.write_all(&[0; 32]).unwrap();
        database.sync_all().unwrap();
        options.max_history_items = 1;
        std::fs::write(
            running_marker_path(&options.database_path),
            b"stale marker\n",
        )
        .unwrap();

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(store.startup_recovery_required());
        assert!(store.recent(10).unwrap().is_empty());
        let report = store.recover_startup().unwrap();
        assert!(report.database_rebuilt);
        assert!(
            report
                .quarantine_path
                .as_ref()
                .is_some_and(|path| path.exists())
        );

        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_quarantine_is_resumed_before_new_store_opens() {
        let root = unique_test_root("clipboard-resumable-quarantine-test");
        std::fs::create_dir_all(&root).unwrap();
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        std::fs::write(&options.database_path, b"damaged database").unwrap();
        std::fs::create_dir_all(&options.payload_directory).unwrap();
        std::fs::write(options.payload_directory.join("must-survive"), b"payload").unwrap();
        std::fs::write(
            running_marker_path(&options.database_path),
            b"stale marker\n",
        )
        .unwrap();

        let token = "123456-1";
        write_quarantine_manifest(
            &quarantine_manifest_path(&options.database_path),
            QuarantinePhase::Moving,
            token,
        )
        .unwrap();
        let quarantined_database = quarantined_database_path(&options, token);
        std::fs::rename(&options.database_path, &quarantined_database).unwrap();

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(store.startup_recovery_required());
        assert!(store.recent(10).unwrap().is_empty());
        let quarantined_payloads = quarantined_payload_path(&options, token);
        assert!(quarantined_payloads.join("must-survive").exists());
        assert!(!options.payload_directory.exists());
        let report = store.recover_startup().unwrap();
        assert_eq!(report.quarantine_path, Some(quarantined_database));
        assert!(!quarantine_manifest_path(&options.database_path).exists());
        assert!(quarantined_payloads.join("must-survive").exists());

        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_startup_recovery_keeps_store_usable_and_retries_next_open() {
        let root = unique_test_root("clipboard-recovery-failure-test");
        std::fs::create_dir_all(&root).unwrap();
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        // A regular file where the payload directory belongs makes the orphan scan fail.
        std::fs::write(&options.payload_directory, b"not a directory").unwrap();
        std::fs::write(
            running_marker_path(&options.database_path),
            b"stale marker\n",
        )
        .unwrap();

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(store.startup_recovery_required());
        assert!(store.recover_startup().is_err());
        assert!(store.recent(10).unwrap().is_empty());
        assert!(store.stats().is_ok());
        store.shutdown().unwrap();
        assert!(running_marker_path(&options.database_path).exists());
        drop(store);

        std::fs::remove_file(&options.payload_directory).unwrap();
        let retried = StoreHandle::open(options.clone()).unwrap();
        assert!(retried.startup_recovery_required());
        assert!(retried.recover_startup().unwrap().was_unclean);
        retried.shutdown().unwrap();
        assert!(!running_marker_path(&options.database_path).exists());
        drop(retried);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_before_recovery_keeps_marker_for_next_open() {
        let root = unique_test_root("clipboard-recovery-deferred-test");
        let options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        drop(StoreHandle::open(options.clone()).unwrap());
        std::fs::write(
            running_marker_path(&options.database_path),
            b"stale marker\n",
        )
        .unwrap();

        let store = StoreHandle::open(options.clone()).unwrap();
        assert!(store.startup_recovery_required());
        store.shutdown().unwrap();
        assert!(running_marker_path(&options.database_path).exists());
        drop(store);

        let reopened = StoreHandle::open(options.clone()).unwrap();
        assert!(reopened.startup_recovery_required());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn maintenance_runs_bounded_gc_checkpoints_and_conditional_fts_optimize() {
        let root = unique_test_root("clipboard-maintenance-test");
        let mut options = StoreOptions::new(root.join("history.sqlite"), root.join("payloads"));
        options.maintenance.passive_checkpoint_wal_bytes = 0;
        options.maintenance.truncate_checkpoint_wal_bytes = 0;
        options.maintenance.vacuum_freelist_pages = 0;
        options.maintenance.fts_optimize_deleted_rows = 1;
        options.maintenance.vacuum_step_pages = 1;
        let store = StoreHandle::open(options.clone()).unwrap();
        let service = HistoryService::new(store, SearchTextPolicy::default());

        let outcome = service
            .capture(
                ClipboardSnapshot {
                    representations: vec![Representation {
                        uti: "public.utf8-plain-text".into(),
                        bytes: b"maintenance fixture".to_vec(),
                    }],
                    image_preview: None,
                },
                ClipKind::Text,
                1,
            )
            .unwrap();
        let CaptureOutcome::Stored(stored) = outcome else {
            panic!("expected stored clip")
        };
        assert!(service.repository().delete(stored.id).unwrap());

        drop(service);
        let reopened = StoreHandle::open(options).unwrap();
        let report = reopened
            .run_maintenance(MaintenanceTrigger::DeepIdle)
            .unwrap();
        assert!(report.fts_optimized);
        assert!(report.truncate_checkpoint.is_some());
        assert_eq!(report.garbage_collection.queued_scanned, 0);

        let second = reopened
            .run_maintenance(MaintenanceTrigger::DeepIdle)
            .unwrap();
        assert!(!second.fts_optimized);

        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_root(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{unique}"))
    }

    fn cursor_for(item: &clipboard_core::ClipSummary) -> HistoryCursor {
        HistoryCursor {
            last_used_at_ms: item.last_used_at_ms,
            id: item.id,
        }
    }
}
