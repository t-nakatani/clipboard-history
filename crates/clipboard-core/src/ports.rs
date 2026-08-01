use crate::{
    ClipCandidate, ClipId, ClipSummary, ImagePreview, PlannedQuery, Representation, UpsertOutcome,
};

pub trait HistoryRepository {
    type Error;

    fn upsert(&self, candidate: ClipCandidate) -> Result<UpsertOutcome, Self::Error>;
    fn recent(&self, limit: usize) -> Result<Vec<ClipSummary>, Self::Error>;
    fn representations(&self, id: ClipId) -> Result<Vec<Representation>, Self::Error>;
    fn image_preview(&self, id: ClipId) -> Result<Option<ImagePreview>, Self::Error>;
    fn search(&self, query: PlannedQuery, limit: usize) -> Result<Vec<ClipSummary>, Self::Error>;
}
