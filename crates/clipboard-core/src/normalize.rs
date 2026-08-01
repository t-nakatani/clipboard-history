#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchTextPolicy {
    pub max_bytes: usize,
}

impl Default for SearchTextPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024,
        }
    }
}

pub fn normalize_search_text(text: &str, policy: SearchTextPolicy) -> Option<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() || policy.max_bytes == 0 {
        return None;
    }

    let mut end = trimmed.len().min(policy.max_bytes);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(trimmed[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_preserves_utf8_boundary() {
        let value = normalize_search_text("あいう", SearchTextPolicy { max_bytes: 4 });
        assert_eq!(value.as_deref(), Some("あ"));
    }
}
