use crate::{
    ClipCandidate, ClipId, ClipSummary, HistoryCursor, HistoryPage, ImagePreview, PlannedQuery,
    Representation, UpsertOutcome,
};

pub trait HistoryRepository {
    type Error;

    fn upsert(&self, candidate: ClipCandidate) -> Result<UpsertOutcome, Self::Error>;
    fn recent_page(
        &self,
        cursor: Option<HistoryCursor>,
        limit: usize,
    ) -> Result<HistoryPage, Self::Error>;
    fn recent(&self, limit: usize) -> Result<Vec<ClipSummary>, Self::Error> {
        self.recent_page(None, limit).map(|page| page.items)
    }
    fn representations(&self, id: ClipId) -> Result<Vec<Representation>, Self::Error>;
    fn image_preview(&self, id: ClipId) -> Result<Option<ImagePreview>, Self::Error>;
    fn search_page(
        &self,
        query: PlannedQuery,
        cursor: Option<HistoryCursor>,
        limit: usize,
    ) -> Result<HistoryPage, Self::Error>;
    fn search(&self, query: PlannedQuery, limit: usize) -> Result<Vec<ClipSummary>, Self::Error> {
        self.search_page(query, None, limit).map(|page| page.items)
    }
}
