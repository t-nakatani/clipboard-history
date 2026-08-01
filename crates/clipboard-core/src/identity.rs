use crate::{ClipIdentity, Representation};

const DOMAIN: &[u8] = b"clipboard-history.clip.v1\0";

pub fn canonical_clip_identity(representations: &[Representation]) -> ClipIdentity {
    let mut ordered: Vec<&Representation> = representations.iter().collect();
    ordered.sort_by(|left, right| {
        left.uti
            .as_bytes()
            .cmp(right.uti.as_bytes())
            .then_with(|| left.bytes.cmp(&right.bytes))
    });

    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(&(ordered.len() as u64).to_le_bytes());
    for representation in ordered {
        hasher.update(&(representation.uti.len() as u64).to_le_bytes());
        hasher.update(representation.uti.as_bytes());
        hasher.update(&(representation.bytes.len() as u64).to_le_bytes());
        hasher.update(&representation.bytes);
    }
    ClipIdentity(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_independent_of_representation_order() {
        let text = Representation {
            uti: "public.utf8-plain-text".into(),
            bytes: b"hello".to_vec(),
        };
        let html = Representation {
            uti: "public.html".into(),
            bytes: b"<b>hello</b>".to_vec(),
        };
        assert_eq!(
            canonical_clip_identity(&[text.clone(), html.clone()]),
            canonical_clip_identity(&[html, text])
        );
    }
}
