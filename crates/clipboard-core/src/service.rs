use crate::{
    CaptureLimits, CaptureOutcome, CaptureRejection, ClipCandidate, ClipKind, ClipboardSnapshot,
    HistoryRepository, MatchMode, QueryPlanner, Representation, SearchTextPolicy,
    canonical_clip_identity, normalize_search_text,
};

pub struct HistoryService<R> {
    repository: R,
    text_policy: SearchTextPolicy,
    capture_limits: CaptureLimits,
}

impl<R: HistoryRepository> HistoryService<R> {
    pub fn new(repository: R, text_policy: SearchTextPolicy) -> Self {
        Self {
            repository,
            text_policy,
            capture_limits: CaptureLimits::default(),
        }
    }

    pub fn with_capture_limits(mut self, capture_limits: CaptureLimits) -> Self {
        self.capture_limits = capture_limits;
        self
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
        // Size limits run before hashing and persistence so oversized clips
        // never reach the repository or the payload store.
        if let Some(rejection) = oversized_rejection(&snapshot.representations, self.capture_limits)
        {
            return Ok(CaptureOutcome::Rejected(rejection));
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

    pub fn capture_limits(&self) -> CaptureLimits {
        self.capture_limits
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

    pub fn search_page(
        &self,
        query: &str,
        mode: MatchMode,
        cursor: Option<crate::HistoryCursor>,
        direction: crate::PageDirection,
        limit: usize,
    ) -> Result<crate::HistoryPage, R::Error> {
        self.repository
            .search_page(QueryPlanner.plan(query, mode), cursor, direction, limit)
    }
}

fn oversized_rejection(
    representations: &[Representation],
    limits: CaptureLimits,
) -> Option<CaptureRejection> {
    let mut total_bytes: u64 = 0;
    for representation in representations {
        let bytes = representation.bytes.len() as u64;
        if bytes > limits.max_representation_bytes as u64 {
            return Some(CaptureRejection::OversizedRepresentation {
                observed_bytes: bytes,
                limit_bytes: limits.max_representation_bytes as u64,
            });
        }
        total_bytes += bytes;
    }
    if total_bytes > limits.max_clip_bytes as u64 {
        return Some(CaptureRejection::OversizedClip {
            observed_bytes: total_bytes,
            limit_bytes: limits.max_clip_bytes as u64,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipId, HistoryPage, ImagePreview, UpsertOutcome};
    use std::cell::RefCell;

    #[derive(Default)]
    struct RecordingRepository {
        upserts: RefCell<Vec<ClipCandidate>>,
    }

    impl HistoryRepository for RecordingRepository {
        type Error = std::convert::Infallible;

        fn upsert(&self, candidate: ClipCandidate) -> Result<UpsertOutcome, Self::Error> {
            self.upserts.borrow_mut().push(candidate);
            Ok(UpsertOutcome {
                id: ClipId(self.upserts.borrow().len() as i64),
                inserted: true,
            })
        }

        fn recent_page(
            &self,
            _cursor: Option<crate::HistoryCursor>,
            _direction: crate::PageDirection,
            _limit: usize,
        ) -> Result<HistoryPage, Self::Error> {
            unimplemented!()
        }

        fn representations(&self, _id: ClipId) -> Result<Vec<Representation>, Self::Error> {
            Ok(Vec::new())
        }

        fn image_preview(&self, _id: ClipId) -> Result<Option<ImagePreview>, Self::Error> {
            Ok(None)
        }

        fn search_page(
            &self,
            _query: crate::PlannedQuery,
            _cursor: Option<crate::HistoryCursor>,
            _direction: crate::PageDirection,
            _limit: usize,
        ) -> Result<HistoryPage, Self::Error> {
            unimplemented!()
        }
    }

    fn service_with_limits(limits: CaptureLimits) -> HistoryService<RecordingRepository> {
        HistoryService::new(RecordingRepository::default(), SearchTextPolicy::default())
            .with_capture_limits(limits)
    }

    fn text_snapshot(bytes: Vec<u8>) -> ClipboardSnapshot {
        ClipboardSnapshot {
            representations: vec![Representation {
                uti: "public.utf8-plain-text".into(),
                bytes,
            }],
            image_preview: None,
        }
    }

    fn capture(
        service: &HistoryService<RecordingRepository>,
        snapshot: ClipboardSnapshot,
    ) -> CaptureOutcome {
        service.capture(snapshot, ClipKind::Text, 1).unwrap()
    }

    #[test]
    fn representation_exactly_at_limit_is_stored() {
        let service = service_with_limits(CaptureLimits {
            max_representation_bytes: 8,
            max_clip_bytes: 8,
        });
        let outcome = capture(&service, text_snapshot(vec![b'a'; 8]));
        assert!(matches!(outcome, CaptureOutcome::Stored(_)));
        assert_eq!(service.repository().upserts.borrow().len(), 1);
    }

    #[test]
    fn representation_one_byte_over_limit_is_rejected_before_persistence() {
        let service = service_with_limits(CaptureLimits {
            max_representation_bytes: 8,
            max_clip_bytes: 64,
        });
        let outcome = capture(&service, text_snapshot(vec![b'a'; 9]));
        assert_eq!(
            outcome,
            CaptureOutcome::Rejected(CaptureRejection::OversizedRepresentation {
                observed_bytes: 9,
                limit_bytes: 8,
            })
        );
        assert!(service.repository().upserts.borrow().is_empty());
    }

    #[test]
    fn multi_representation_clip_over_total_limit_is_rejected() {
        let service = service_with_limits(CaptureLimits {
            max_representation_bytes: 8,
            max_clip_bytes: 12,
        });
        let snapshot = ClipboardSnapshot {
            representations: vec![
                Representation {
                    uti: "public.utf8-plain-text".into(),
                    bytes: vec![b'a'; 7],
                },
                Representation {
                    uti: "public.html".into(),
                    bytes: vec![b'b'; 6],
                },
            ],
            image_preview: None,
        };
        let outcome = capture(&service, snapshot);
        assert_eq!(
            outcome,
            CaptureOutcome::Rejected(CaptureRejection::OversizedClip {
                observed_bytes: 13,
                limit_bytes: 12,
            })
        );
        assert!(service.repository().upserts.borrow().is_empty());
    }

    #[test]
    fn multi_representation_clip_exactly_at_total_limit_is_stored() {
        let service = service_with_limits(CaptureLimits {
            max_representation_bytes: 8,
            max_clip_bytes: 13,
        });
        let snapshot = ClipboardSnapshot {
            representations: vec![
                Representation {
                    uti: "public.utf8-plain-text".into(),
                    bytes: vec![b'a'; 7],
                },
                Representation {
                    uti: "public.html".into(),
                    bytes: vec![b'b'; 6],
                },
            ],
            image_preview: None,
        };
        assert!(matches!(
            capture(&service, snapshot),
            CaptureOutcome::Stored(_)
        ));
    }

    #[test]
    fn repeated_oversized_captures_never_reach_the_repository() {
        let service = service_with_limits(CaptureLimits {
            max_representation_bytes: 4,
            max_clip_bytes: 4,
        });
        for _ in 0..5 {
            let outcome = capture(&service, text_snapshot(vec![b'x'; 5]));
            assert!(matches!(outcome, CaptureOutcome::Rejected(_)));
        }
        assert!(service.repository().upserts.borrow().is_empty());
    }
}
