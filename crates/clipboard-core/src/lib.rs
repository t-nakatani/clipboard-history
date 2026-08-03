mod filter;
mod identity;
mod model;
mod normalize;
mod ports;
mod query;
mod service;

pub use filter::{CaptureFilter, FilterReason};
pub use identity::canonical_clip_identity;
pub use model::{
    CaptureLimits, CaptureOutcome, CaptureRejection, ClipCandidate, ClipId, ClipIdentity, ClipKind,
    ClipSummary, ClipboardSnapshot, HistoryCursor, HistoryPage, ImagePreview, PageDirection,
    Representation, UpsertOutcome,
};
pub use normalize::{SearchTextPolicy, normalize_search_text};
pub use ports::HistoryRepository;
pub use query::{MatchMode, PlannedQuery, QueryPlanner};
pub use service::HistoryService;
