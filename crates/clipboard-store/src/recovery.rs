use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;

use crate::{
    GarbageCollectionStats, PayloadStore, StoreError,
    actor::{StoreOptions, open_initialized_store, quick_check},
    gc, repository,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StartupRecoveryReport {
    pub was_unclean: bool,
    pub quick_check_passed: bool,
    pub database_rebuilt: bool,
    pub quarantine_path: Option<PathBuf>,
    pub garbage_collection: GarbageCollectionStats,
}

/// A failed recovery attempt. `connection_usable` is false only when the store connection was
/// left unusable, which is the sole reason to stop the actor instead of serving later commands.
pub(crate) struct RecoveryFailure {
    pub(crate) error: StoreError,
    pub(crate) connection_usable: bool,
}

impl RecoveryFailure {
    pub(crate) const fn retryable(error: StoreError) -> Self {
        Self {
            error,
            connection_usable: true,
        }
    }

    pub(crate) const fn fatal(error: StoreError) -> Self {
        Self {
            error,
            connection_usable: false,
        }
    }
}

pub(crate) fn perform_startup_recovery(
    connection: &mut Connection,
    payload_store: &mut PayloadStore,
    options: &StoreOptions,
    initial_quarantine: &mut Option<PathBuf>,
) -> Result<(StartupRecoveryReport, Option<usize>), RecoveryFailure> {
    if initial_quarantine.is_some() {
        quick_check(connection).map_err(RecoveryFailure::fatal)?;
        let garbage_collection = collect_startup_garbage(connection, payload_store)
            .map_err(RecoveryFailure::retryable)?;
        let live_count = repository::count(connection).map_err(RecoveryFailure::retryable)?;
        finish_quarantine(options).map_err(RecoveryFailure::retryable)?;
        return Ok((
            StartupRecoveryReport {
                was_unclean: true,
                quick_check_passed: false,
                database_rebuilt: true,
                quarantine_path: initial_quarantine.take(),
                garbage_collection,
            },
            Some(live_count),
        ));
    }

    match quick_check(connection) {
        Ok(()) => {
            let garbage_collection = collect_startup_garbage(connection, payload_store)
                .map_err(RecoveryFailure::retryable)?;
            Ok((
                StartupRecoveryReport {
                    was_unclean: true,
                    quick_check_passed: true,
                    database_rebuilt: false,
                    quarantine_path: None,
                    garbage_collection,
                },
                None,
            ))
        }
        Err(_) => {
            // From here the live connection is replaced, so any failure leaves the actor without
            // a usable store.
            let placeholder = Connection::open_in_memory()
                .map_err(|error| RecoveryFailure::fatal(StoreError::from(error)))?;
            let broken = std::mem::replace(connection, placeholder);
            drop(broken);
            let quarantine_path = quarantine_store(options).map_err(RecoveryFailure::fatal)?;
            *initial_quarantine = Some(quarantine_path.clone());
            let (new_connection, new_payload_store, live_count) =
                open_initialized_store(options).map_err(RecoveryFailure::fatal)?;
            *connection = new_connection;
            *payload_store = new_payload_store;
            quick_check(connection).map_err(RecoveryFailure::fatal)?;
            finish_quarantine(options).map_err(RecoveryFailure::retryable)?;
            initial_quarantine.take();
            Ok((
                StartupRecoveryReport {
                    was_unclean: true,
                    quick_check_passed: false,
                    database_rebuilt: true,
                    quarantine_path: Some(quarantine_path),
                    garbage_collection: GarbageCollectionStats::default(),
                },
                Some(live_count),
            ))
        }
    }
}

pub(crate) fn collect_startup_garbage(
    connection: &mut Connection,
    payload_store: &PayloadStore,
) -> Result<GarbageCollectionStats, StoreError> {
    let mut total = GarbageCollectionStats::default();
    loop {
        let batch = gc::collect_queued(connection, payload_store, 10_000)?;
        add_gc_stats(&mut total, batch);
        if batch.queued_scanned < 10_000 {
            break;
        }
    }
    add_gc_stats(&mut total, gc::collect_orphans(connection, payload_store)?);
    Ok(total)
}

pub(crate) fn add_gc_stats(total: &mut GarbageCollectionStats, batch: GarbageCollectionStats) {
    total.queued_scanned += batch.queued_scanned;
    total.referenced_skipped += batch.referenced_skipped;
    total.payload_files_deleted += batch.payload_files_deleted;
    total.missing_payload_files += batch.missing_payload_files;
    total.orphan_files_deleted += batch.orphan_files_deleted;
    total.staged_files_deleted += batch.staged_files_deleted;
}

pub(crate) fn running_marker_path(database_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.running", database_path.display()))
}

/// Returns true when a marker from a previous process already existed.
pub(crate) fn mark_store_running(marker_path: &Path) -> Result<bool, StoreError> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker_path)
    {
        Ok(mut file) => {
            writeln!(file, "pid={}", std::process::id())?;
            file.sync_all()?;
            if let Some(parent) = marker_path.parent() {
                sync_directory(parent)?;
            }
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(true),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn remove_running_marker(marker_path: &Path) -> Result<(), StoreError> {
    match fs::remove_file(marker_path) {
        Ok(()) => {
            if let Some(parent) = marker_path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuarantinePhase {
    Moving,
    Moved,
}

pub(crate) fn quarantine_manifest_path(database_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.quarantine", database_path.display()))
}

pub(crate) fn quarantine_store(options: &StoreOptions) -> Result<PathBuf, StoreError> {
    let manifest = quarantine_manifest_path(&options.database_path);
    let (phase, token) = if manifest.exists() {
        read_quarantine_manifest(&manifest)?
    } else {
        let token = unique_quarantine_token(options);
        write_quarantine_manifest(&manifest, QuarantinePhase::Moving, &token)?;
        (QuarantinePhase::Moving, token)
    };
    let quarantine_path = quarantined_database_path(options, &token);
    if phase == QuarantinePhase::Moving {
        move_quarantine_components(options, &token)?;
        write_quarantine_manifest(&manifest, QuarantinePhase::Moved, &token)?;
    }
    if !quarantine_path.exists() {
        return Err(StoreError::InvalidData(
            "quarantine manifest points to a missing database",
        ));
    }
    Ok(quarantine_path)
}

pub(crate) fn resume_quarantine_if_needed(
    options: &StoreOptions,
) -> Result<Option<PathBuf>, StoreError> {
    let manifest = quarantine_manifest_path(&options.database_path);
    if !manifest.exists() {
        return Ok(None);
    }
    quarantine_store(options).map(Some)
}

pub(crate) fn finish_quarantine(options: &StoreOptions) -> Result<(), StoreError> {
    let manifest = quarantine_manifest_path(&options.database_path);
    remove_running_marker(&manifest)
}

pub(crate) fn unique_quarantine_token(options: &StoreOptions) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for sequence in 0_u32.. {
        let token = if sequence == 0 {
            timestamp.to_string()
        } else {
            format!("{timestamp}-{sequence}")
        };
        if !quarantined_database_path(options, &token).exists()
            && !quarantined_payload_path(options, &token).exists()
        {
            return token;
        }
    }
    unreachable!("u32 quarantine sequence exhausted")
}

pub(crate) fn quarantined_database_path(options: &StoreOptions, token: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}.corrupt-{token}",
        options.database_path.display()
    ))
}

pub(crate) fn quarantined_payload_path(options: &StoreOptions, token: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}.corrupt-{token}",
        options.payload_directory.display()
    ))
}

pub(crate) fn move_quarantine_components(
    options: &StoreOptions,
    token: &str,
) -> Result<(), StoreError> {
    let quarantine_path = quarantined_database_path(options, token);
    move_if_needed(&options.database_path, &quarantine_path, true)?;
    for suffix in ["-wal", "-shm"] {
        move_if_needed(
            &PathBuf::from(format!("{}{suffix}", options.database_path.display())),
            &PathBuf::from(format!("{}{suffix}", quarantine_path.display())),
            false,
        )?;
    }
    move_if_needed(
        &options.payload_directory,
        &quarantined_payload_path(options, token),
        false,
    )?;
    if let Some(parent) = options.database_path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

pub(crate) fn move_if_needed(
    source: &Path,
    destination: &Path,
    required: bool,
) -> Result<(), StoreError> {
    match (source.exists(), destination.exists()) {
        (true, false) => fs::rename(source, destination).map_err(Into::into),
        (false, true) | (false, false) if !required => Ok(()),
        (false, true) => Ok(()),
        (false, false) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("quarantine source is missing: {}", source.display()),
        )
        .into()),
        (true, true) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "quarantine source and destination both exist: {} and {}",
                source.display(),
                destination.display()
            ),
        )
        .into()),
    }
}

pub(crate) fn read_quarantine_manifest(
    path: &Path,
) -> Result<(QuarantinePhase, String), StoreError> {
    let contents = fs::read_to_string(path)?;
    let (phase, token) = contents
        .trim()
        .split_once(':')
        .ok_or(StoreError::InvalidData("invalid quarantine manifest"))?;
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
    {
        return Err(StoreError::InvalidData("invalid quarantine token"));
    }
    let phase = match phase {
        "moving" => QuarantinePhase::Moving,
        "moved" => QuarantinePhase::Moved,
        _ => return Err(StoreError::InvalidData("invalid quarantine phase")),
    };
    Ok((phase, token.to_owned()))
}

pub(crate) fn write_quarantine_manifest(
    path: &Path,
    phase: QuarantinePhase,
    token: &str,
) -> Result<(), StoreError> {
    let phase = match phase {
        QuarantinePhase::Moving => "moving",
        QuarantinePhase::Moved => "moved",
    };
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    writeln!(file, "{phase}:{token}")?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(crate) fn is_corrupt_database_error(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::Database(rusqlite::Error::SqliteFailure(code, _))
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            )
    )
}
