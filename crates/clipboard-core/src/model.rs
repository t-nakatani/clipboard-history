#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClipId(pub i64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipKind {
    Text,
    Image,
    File,
    Mixed,
}

impl ClipKind {
    pub const fn as_i64(self) -> i64 {
        match self {
            Self::Text => 0,
            Self::Image => 1,
            Self::File => 2,
            Self::Mixed => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Representation {
    pub uti: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImagePreview {
    pub uti: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardSnapshot {
    pub representations: Vec<Representation>,
    /// A derived, bounded thumbnail. It is presentation data and is excluded
    /// from canonical clip identity and pasteboard restoration.
    pub image_preview: Option<ImagePreview>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClipIdentity(pub [u8; 32]);

impl ClipIdentity {
    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipCandidate {
    pub identity: ClipIdentity,
    pub kind: ClipKind,
    pub copied_at_ms: i64,
    pub normalized_text: Option<String>,
    pub representations: Vec<Representation>,
    pub image_preview: Option<ImagePreview>,
}

impl ClipCandidate {
    pub fn payload_size(&self) -> u64 {
        self.representations
            .iter()
            .map(|representation| representation.bytes.len() as u64)
            .sum()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipSummary {
    pub id: ClipId,
    pub kind: ClipKind,
    pub last_used_at_ms: i64,
    pub pinned: bool,
    pub copy_count: u64,
    pub payload_size: u64,
    pub preview: Option<String>,
    pub has_image_preview: bool,
}

/// Stable seek position for history ordered by `(last_used_at, id) DESC`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryCursor {
    pub last_used_at_ms: i64,
    pub id: ClipId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageDirection {
    Older,
    Newer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    pub items: Vec<ClipSummary>,
    /// Seek position for the next request in the same direction. This can be
    /// beyond the last matching item when a bounded recent scan is truncated.
    pub continuation_cursor: Option<HistoryCursor>,
    pub has_more: bool,
    /// The bounded scan reached its row budget before proving end-of-history.
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpsertOutcome {
    pub id: ClipId,
    pub inserted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureOutcome {
    Stored(UpsertOutcome),
    Empty,
    /// The snapshot violated a capture size limit and was rejected before
    /// hashing or persistence.
    Rejected(CaptureRejection),
}

/// Why a snapshot was rejected at the capture boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureRejection {
    OversizedRepresentation {
        observed_bytes: u64,
        limit_bytes: u64,
    },
    OversizedClip {
        observed_bytes: u64,
        limit_bytes: u64,
    },
}

/// Size limits enforced before a snapshot is hashed or persisted.
///
/// `max_clip_bytes` must not exceed the store's restore byte limit so that
/// every stored clip stays restorable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureLimits {
    pub max_representation_bytes: usize,
    pub max_clip_bytes: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        // Mirrors the store's default `max_restore_bytes`.
        Self {
            max_representation_bytes: 64 * 1024 * 1024,
            max_clip_bytes: 64 * 1024 * 1024,
        }
    }
}
