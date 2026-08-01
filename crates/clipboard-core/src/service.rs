use crate::{
    CaptureOutcome, ClipCandidate, ClipKind, ClipboardSnapshot, HistoryRepository, MatchMode,
    QueryPlanner, Representation, SearchTextPolicy, canonical_clip_identity, normalize_search_text,
};

pub struct HistoryService<R> {
    repository: R,
    text_policy: SearchTextPolicy,
}

impl<R: HistoryRepository> HistoryService<R> {
    pub fn new(repository: R, text_policy: SearchTextPolicy) -> Self {
        Self {
            repository,
            text_policy,
        }
    }

    pub fn capture(
        &self,
        snapshot: ClipboardSnapshot,
        kind: ClipKind,
        copied_at_ms: i64,
    ) -> Result<CaptureOutcome, R::Error> {
        if snapshot.representations.is_empty() {
            return Ok(CaptureOutcome::Empty);
        }
        let normalized_text = snapshot
            .representations
            .iter()
            .find(|representation| representation.uti == "public.utf8-plain-text")
            .and_then(|representation| std::str::from_utf8(&representation.bytes).ok())
            .and_then(|text| normalize_search_text(text, self.text_policy));
        let identity = canonical_clip_identity(&snapshot.representations);
        let outcome = self.repository.upsert(ClipCandidate {
            identity,
            kind,
            copied_at_ms,
            normalized_text,
            representations: snapshot.representations,
            image_preview: snapshot.image_preview,
        })?;
        Ok(CaptureOutcome::Stored(outcome))
    }

    pub fn repository(&self) -> &R {
        &self.repository
    }

    pub fn select(&self, id: crate::ClipId) -> Result<Vec<Representation>, R::Error> {
        self.repository.representations(id)
    }

    pub fn image_preview(
        &self,
        id: crate::ClipId,
    ) -> Result<Option<crate::ImagePreview>, R::Error> {
        self.repository.image_preview(id)
    }

    pub fn search(
        &self,
        query: &str,
        mode: MatchMode,
        limit: usize,
    ) -> Result<Vec<crate::ClipSummary>, R::Error> {
        self.repository
            .search(QueryPlanner.plan(query, mode), limit)
    }
}
