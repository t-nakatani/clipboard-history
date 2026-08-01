const CONCEALED_TYPES: &[&str] = &[
    "org.nspasteboard.ConcealedType",
    "com.agilebits.onepassword",
];
const TRANSIENT_TYPES: &[&str] = &["org.nspasteboard.TransientType"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterReason {
    Concealed,
    Transient,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureFilter;

impl CaptureFilter {
    /// Evaluates the pasteboard's advertised types before any payload bytes are read.
    pub fn evaluate_types(&self, pasteboard_types: &[String]) -> Result<(), FilterReason> {
        for pasteboard_type in pasteboard_types {
            if CONCEALED_TYPES.contains(&pasteboard_type.as_str()) {
                return Err(FilterReason::Concealed);
            }
            if TRANSIENT_TYPES.contains(&pasteboard_type.as_str()) {
                return Err(FilterReason::Transient);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concealed_marker_is_rejected_without_becoming_a_representation() {
        let advertised_types = vec![
            "org.nspasteboard.ConcealedType".to_owned(),
            "public.utf8-plain-text".to_owned(),
        ];
        assert_eq!(
            CaptureFilter.evaluate_types(&advertised_types),
            Err(FilterReason::Concealed)
        );
    }

    #[test]
    fn ordinary_storage_types_are_accepted() {
        let advertised_types = vec![
            "public.utf8-plain-text".to_owned(),
            "public.html".to_owned(),
        ];
        assert_eq!(CaptureFilter.evaluate_types(&advertised_types), Ok(()));
    }
}
