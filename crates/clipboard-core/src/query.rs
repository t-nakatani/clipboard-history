#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchMode {
    Exact,
    Prefix,
    Substring,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlannedQuery {
    Empty,
    RecentScan { mode: MatchMode, needle: String },
    Indexed { mode: MatchMode, needle: String },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct QueryPlanner;

impl QueryPlanner {
    pub fn plan(&self, query: &str, mode: MatchMode) -> PlannedQuery {
        let needle = query.trim();
        if needle.is_empty() {
            return PlannedQuery::Empty;
        }
        if matches!(mode, MatchMode::Exact | MatchMode::Substring) && needle.chars().count() < 3 {
            return PlannedQuery::RecentScan {
                mode,
                needle: needle.to_owned(),
            };
        }
        PlannedQuery::Indexed {
            mode,
            needle: needle.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_non_prefix_queries_keep_mode_in_bounded_recent_scan() {
        assert_eq!(
            QueryPlanner.plan("ab", MatchMode::Exact),
            PlannedQuery::RecentScan {
                mode: MatchMode::Exact,
                needle: "ab".into(),
            }
        );
        assert_eq!(
            QueryPlanner.plan("ab", MatchMode::Substring),
            PlannedQuery::RecentScan {
                mode: MatchMode::Substring,
                needle: "ab".into(),
            }
        );
        assert!(matches!(
            QueryPlanner.plan("ab", MatchMode::Prefix),
            PlannedQuery::Indexed {
                mode: MatchMode::Prefix,
                ..
            }
        ));
    }
}
