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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    pub items: Vec<ClipSummary>,
    pub next_cursor: Option<HistoryCursor>,
    pub has_more: bool,
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
}
