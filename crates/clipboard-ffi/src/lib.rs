use std::sync::Arc;

use clipboard_core::{
    CaptureFilter, CaptureOutcome, ClipKind, ClipboardSnapshot, FilterReason, HistoryRepository,
    HistoryService, Representation, SearchTextPolicy, canonical_clip_identity,
};
use clipboard_store::{StoreHandle, StoreOptions};

uniffi::setup_scaffolding!();

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct RepresentationDto {
    pub uti: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct CaptureResultDto {
    pub id: i64,
    pub inserted: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ClipSummaryDto {
    pub id: i64,
    pub kind: String,
    pub last_used_at_ms: i64,
    pub pinned: bool,
    pub copy_count: u64,
    pub payload_size: u64,
    pub preview: Option<String>,
    pub has_image_preview: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct HistoryCursorDto {
    pub last_used_at_ms: i64,
    pub id: i64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct HistoryPageDto {
    pub items: Vec<ClipSummaryDto>,
    pub continuation_cursor: Option<HistoryCursorDto>,
    pub has_more: bool,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum PageDirectionDto {
    Older,
    Newer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum SearchModeDto {
    Exact,
    Prefix,
    Substring,
}

#[derive(Debug, uniffi::Error)]
pub enum ClipboardFfiError {
    Store { message: String },
    InvalidInput { message: String },
}

impl std::fmt::Display for ClipboardFfiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store { message } => write!(formatter, "store error: {message}"),
            Self::InvalidInput { message } => write!(formatter, "invalid input: {message}"),
        }
    }
}

impl std::error::Error for ClipboardFfiError {}

#[derive(uniffi::Object)]
pub struct ClipboardEngine {
    service: HistoryService<StoreHandle>,
}

#[uniffi::export]
impl ClipboardEngine {
    #[uniffi::constructor]
    pub fn open(
        database_path: String,
        payload_directory: String,
    ) -> Result<Arc<Self>, ClipboardFfiError> {
        if database_path.is_empty() || payload_directory.is_empty() {
            return Err(ClipboardFfiError::InvalidInput {
                message: "database and payload paths must not be empty".into(),
            });
        }
        let options = StoreOptions::new(database_path, payload_directory);
        let repository = StoreHandle::open(options).map_err(store_error)?;
        repository.quick_check().map_err(store_error)?;
        Ok(Arc::new(Self {
            service: HistoryService::new(repository, SearchTextPolicy::default()),
        }))
    }

    pub fn capture(
        &self,
        representations: Vec<RepresentationDto>,
        image_preview: Option<RepresentationDto>,
        copied_at_ms: i64,
    ) -> Result<CaptureResultDto, ClipboardFfiError> {
        if representations.is_empty() {
            return Err(ClipboardFfiError::InvalidInput {
                message: "at least one storage representation is required".into(),
            });
        }
        let representations: Vec<Representation> =
            representations.into_iter().map(Into::into).collect();
        let kind = classify_kind(&representations);
        let image_preview = image_preview.map(|preview| clipboard_core::ImagePreview {
            uti: preview.uti,
            bytes: preview.bytes,
        });
        match self
            .service
            .capture(
                ClipboardSnapshot {
                    representations,
                    image_preview,
                },
                kind,
                copied_at_ms,
            )
            .map_err(store_error)?
        {
            CaptureOutcome::Stored(outcome) => Ok(CaptureResultDto {
                id: outcome.id.0,
                inserted: outcome.inserted,
            }),
            CaptureOutcome::Empty => Err(ClipboardFfiError::InvalidInput {
                message: "empty snapshot was not persisted".into(),
            }),
        }
    }

    pub fn recent(&self, limit: u32) -> Result<Vec<ClipSummaryDto>, ClipboardFfiError> {
        self.recent_page(None, PageDirectionDto::Older, limit)
            .map(|page| page.items)
    }

    pub fn recent_page(
        &self,
        cursor: Option<HistoryCursorDto>,
        direction: PageDirectionDto,
        limit: u32,
    ) -> Result<HistoryPageDto, ClipboardFfiError> {
        self.service
            .repository()
            .recent_page(
                cursor.map(history_cursor),
                page_direction(direction),
                limit.clamp(1, 200) as usize,
            )
            .map_err(store_error)
            .map(history_page_dto)
    }

    pub fn delete(&self, id: i64) -> Result<bool, ClipboardFfiError> {
        let deleted = self
            .service
            .repository()
            .delete(clipboard_core::ClipId(id))
            .map_err(store_error)?;
        if deleted {
            // Physical payload deletion remains off the SQL delete path.
            self.service
                .repository()
                .collect_garbage(100)
                .map_err(store_error)?;
        }
        Ok(deleted)
    }

    pub fn select(&self, id: i64) -> Result<Vec<RepresentationDto>, ClipboardFfiError> {
        self.service
            .select(clipboard_core::ClipId(id))
            .map_err(store_error)
            .map(|representations| {
                representations
                    .into_iter()
                    .map(|representation| RepresentationDto {
                        uti: representation.uti,
                        bytes: representation.bytes,
                    })
                    .collect()
            })
    }

    pub fn image_preview(&self, id: i64) -> Result<Option<RepresentationDto>, ClipboardFfiError> {
        self.service
            .image_preview(clipboard_core::ClipId(id))
            .map_err(store_error)
            .map(|preview| {
                preview.map(|preview| RepresentationDto {
                    uti: preview.uti,
                    bytes: preview.bytes,
                })
            })
    }

    pub fn search(
        &self,
        query: String,
        mode: SearchModeDto,
        limit: u32,
    ) -> Result<Vec<ClipSummaryDto>, ClipboardFfiError> {
        self.search_page(query, mode, None, PageDirectionDto::Older, limit)
            .map(|page| page.items)
    }

    pub fn search_page(
        &self,
        query: String,
        mode: SearchModeDto,
        cursor: Option<HistoryCursorDto>,
        direction: PageDirectionDto,
        limit: u32,
    ) -> Result<HistoryPageDto, ClipboardFfiError> {
        let mode = match mode {
            SearchModeDto::Exact => clipboard_core::MatchMode::Exact,
            SearchModeDto::Prefix => clipboard_core::MatchMode::Prefix,
            SearchModeDto::Substring => clipboard_core::MatchMode::Substring,
        };
        self.service
            .search_page(
                &query,
                mode,
                cursor.map(history_cursor),
                page_direction(direction),
                limit.clamp(1, 200) as usize,
            )
            .map_err(store_error)
            .map(history_page_dto)
    }
}

fn clip_summary_dto(row: clipboard_core::ClipSummary) -> ClipSummaryDto {
    ClipSummaryDto {
        id: row.id.0,
        kind: kind_name(row.kind).into(),
        last_used_at_ms: row.last_used_at_ms,
        pinned: row.pinned,
        copy_count: row.copy_count,
        payload_size: row.payload_size,
        preview: row.preview,
        has_image_preview: row.has_image_preview,
    }
}

fn history_cursor(cursor: HistoryCursorDto) -> clipboard_core::HistoryCursor {
    clipboard_core::HistoryCursor {
        last_used_at_ms: cursor.last_used_at_ms,
        id: clipboard_core::ClipId(cursor.id),
    }
}

fn page_direction(direction: PageDirectionDto) -> clipboard_core::PageDirection {
    match direction {
        PageDirectionDto::Older => clipboard_core::PageDirection::Older,
        PageDirectionDto::Newer => clipboard_core::PageDirection::Newer,
    }
}

fn history_page_dto(page: clipboard_core::HistoryPage) -> HistoryPageDto {
    HistoryPageDto {
        items: page.items.into_iter().map(clip_summary_dto).collect(),
        continuation_cursor: page.continuation_cursor.map(|cursor| HistoryCursorDto {
            last_used_at_ms: cursor.last_used_at_ms,
            id: cursor.id.0,
        }),
        has_more: page.has_more,
        truncated: page.truncated,
    }
}

fn store_error(error: clipboard_store::StoreError) -> ClipboardFfiError {
    ClipboardFfiError::Store {
        message: error.to_string(),
    }
}

fn classify_kind(representations: &[Representation]) -> ClipKind {
    let has_text = representations.iter().any(|value| {
        value.uti == "public.utf8-plain-text"
            || value.uti == "public.html"
            || value.uti == "public.rtf"
    });
    let has_image = representations
        .iter()
        .any(|value| value.uti == "public.png" || value.uti == "public.tiff");
    let has_file = representations
        .iter()
        .any(|value| value.uti == "public.file-url");
    match (has_text, has_image, has_file) {
        (true, false, false) => ClipKind::Text,
        (false, true, false) => ClipKind::Image,
        (false, false, true) => ClipKind::File,
        _ => ClipKind::Mixed,
    }
}

const fn kind_name(kind: ClipKind) -> &'static str {
    match kind {
        ClipKind::Text => "text",
        ClipKind::Image => "image",
        ClipKind::File => "file",
        ClipKind::Mixed => "mixed",
    }
}

impl From<RepresentationDto> for Representation {
    fn from(value: RepresentationDto) -> Self {
        Self {
            uti: value.uti,
            bytes: value.bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum CaptureFilterDecisionDto {
    Accept,
    RejectConcealed,
    RejectTransient,
}

#[uniffi::export]
pub fn evaluate_capture_types(pasteboard_types: Vec<String>) -> CaptureFilterDecisionDto {
    match CaptureFilter.evaluate_types(&pasteboard_types) {
        Ok(()) => CaptureFilterDecisionDto::Accept,
        Err(FilterReason::Concealed) => CaptureFilterDecisionDto::RejectConcealed,
        Err(FilterReason::Transient) => CaptureFilterDecisionDto::RejectTransient,
    }
}

#[uniffi::export]
pub fn canonical_hash(representations: Vec<RepresentationDto>) -> String {
    let representations: Vec<Representation> =
        representations.into_iter().map(Into::into).collect();
    canonical_clip_identity(&representations).to_hex()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn ffi_hash_is_stable_across_order() {
        let text = RepresentationDto {
            uti: "public.utf8-plain-text".into(),
            bytes: b"hello".to_vec(),
        };
        let html = RepresentationDto {
            uti: "public.html".into(),
            bytes: b"<b>hello</b>".to_vec(),
        };
        assert_eq!(
            canonical_hash(vec![text.clone(), html.clone()]),
            canonical_hash(vec![html, text])
        );
    }

    #[test]
    fn ffi_filter_rejects_marker_types_before_payload_dtos_exist() {
        assert_eq!(
            evaluate_capture_types(vec![
                "public.utf8-plain-text".into(),
                "org.nspasteboard.ConcealedType".into(),
            ]),
            CaptureFilterDecisionDto::RejectConcealed
        );
    }

    #[test]
    fn engine_persists_recopies_and_deletes() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-ffi-test-{unique}"));
        let engine = ClipboardEngine::open(
            root.join("history.sqlite").to_string_lossy().into_owned(),
            root.join("payloads").to_string_lossy().into_owned(),
        )
        .unwrap();
        let representation = RepresentationDto {
            uti: "public.utf8-plain-text".into(),
            bytes: b"persisted through ffi".to_vec(),
        };

        let inserted = engine
            .capture(vec![representation.clone()], None, 10)
            .unwrap();
        assert!(inserted.inserted);
        let preview = RepresentationDto {
            uti: "public.png".into(),
            bytes: vec![1, 2, 3],
        };
        let touched = engine
            .capture(vec![representation], Some(preview.clone()), 20)
            .unwrap();
        assert_eq!(touched.id, inserted.id);
        assert!(!touched.inserted);

        let rows = engine.recent(50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].copy_count, 2);
        assert_eq!(rows[0].preview.as_deref(), Some("persisted through ffi"));
        assert!(rows[0].has_image_preview);
        assert_eq!(engine.image_preview(rows[0].id).unwrap(), Some(preview));
        let selected = engine.select(rows[0].id).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].bytes, b"persisted through ffi");
        let searched = engine
            .search("persisted".into(), SearchModeDto::Prefix, 50)
            .unwrap();
        assert_eq!(searched.len(), 1);
        assert!(engine.delete(rows[0].id).unwrap());
        assert!(engine.recent(50).unwrap().is_empty());

        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ffi_exposes_cursor_and_has_more_for_recent_and_search() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-ffi-page-test-{unique}"));
        let engine = ClipboardEngine::open(
            root.join("history.sqlite").to_string_lossy().into_owned(),
            root.join("payloads").to_string_lossy().into_owned(),
        )
        .unwrap();
        for index in 0..5 {
            engine
                .capture(
                    vec![RepresentationDto {
                        uti: "public.utf8-plain-text".into(),
                        bytes: format!("page-{index}").into_bytes(),
                    }],
                    None,
                    10 - index,
                )
                .unwrap();
        }

        let first = engine
            .recent_page(None, PageDirectionDto::Older, 2)
            .unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.has_more);
        assert!(!first.truncated);
        let second = engine
            .recent_page(first.continuation_cursor, PageDirectionDto::Older, 2)
            .unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(second.has_more);
        let final_page = engine
            .recent_page(second.continuation_cursor, PageDirectionDto::Older, 2)
            .unwrap();
        assert_eq!(final_page.items.len(), 1);
        assert!(!final_page.has_more);

        let newer = engine
            .recent_page(
                final_page.items.first().map(dto_cursor),
                PageDirectionDto::Newer,
                2,
            )
            .unwrap();
        assert_eq!(newer.items.len(), 2);
        assert!(newer.has_more);

        let search = engine
            .search_page(
                "page-".into(),
                SearchModeDto::Prefix,
                None,
                PageDirectionDto::Older,
                2,
            )
            .unwrap();
        assert_eq!(search.items.len(), 2);
        assert!(search.has_more);

        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn dto_cursor(item: &ClipSummaryDto) -> HistoryCursorDto {
        HistoryCursorDto {
            last_used_at_ms: item.last_used_at_ms,
            id: item.id,
        }
    }
}
