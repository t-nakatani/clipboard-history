/// How the planner asks the index to match a needle.
///
/// `Prefix` is answerable by a seek at any needle length. `Substring` needs
/// three characters before an index can answer it, and falls back to a bounded
/// recent scan below that.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchMode {
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
    /// Turns raw search text into the query the repository should run.
    ///
    /// Callers pass text, not a match mode: history search is always substring,
    /// and picking between the modes is the planner's job because only it knows
    /// what each one costs at a given needle length.
    pub fn plan(&self, query: &str) -> PlannedQuery {
        let needle = query.trim();
        if needle.is_empty() {
            return PlannedQuery::Empty;
        }
        if needle.chars().count() < 3 {
            return PlannedQuery::RecentScan {
                mode: MatchMode::Substring,
                needle: needle.to_owned(),
            };
        }
        PlannedQuery::Indexed {
            mode: MatchMode::Substring,
            needle: needle.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needles_below_the_trigram_floor_fall_back_to_a_bounded_recent_scan() {
        assert_eq!(
            QueryPlanner.plan("ab"),
            PlannedQuery::RecentScan {
                mode: MatchMode::Substring,
                needle: "ab".into(),
            }
        );
        assert_eq!(
            QueryPlanner.plan("abc"),
            PlannedQuery::Indexed {
                mode: MatchMode::Substring,
                needle: "abc".into(),
            }
        );
    }

    #[test]
    fn blank_text_plans_no_query_at_all() {
        assert_eq!(QueryPlanner.plan("   "), PlannedQuery::Empty);
    }
}
