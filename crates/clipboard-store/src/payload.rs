use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::StoreError;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PayloadHash(pub [u8; 32]);

impl PayloadHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

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
pub struct StoredPayload {
    pub hash: PayloadHash,
    pub size: u64,
    pub path: PathBuf,
    pub created: bool,
}

#[derive(Clone, Debug)]
pub struct PayloadStore {
    root: PathBuf,
}

impl PayloadStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn put(&self, bytes: &[u8]) -> Result<StoredPayload, StoreError> {
        let hash = PayloadHash::of(bytes);
        let destination = self.path_for(hash);
        if destination.exists() {
            return Ok(StoredPayload {
                hash,
                size: bytes.len() as u64,
                path: destination,
                created: false,
            });
        }

        let parent = destination
            .parent()
            .ok_or(StoreError::InvalidData("payload path has no parent"))?;
        fs::create_dir_all(parent)?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".stage-{}-{sequence}", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);

        match fs::rename(&temp, &destination) {
            Ok(()) => {
                sync_directory(parent)?;
                Ok(StoredPayload {
                    hash,
                    size: bytes.len() as u64,
                    path: destination,
                    created: true,
                })
            }
            Err(_error) if destination.exists() => {
                let _ = fs::remove_file(&temp);
                Ok(StoredPayload {
                    hash,
                    size: bytes.len() as u64,
                    path: destination,
                    created: false,
                })
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                Err(error.into())
            }
        }
    }

    pub fn path_for(&self, hash: PayloadHash) -> PathBuf {
        let hex = hash.to_hex();
        self.root.join(&hex[..2]).join(&hex[2..4]).join(hex)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn remove_if_exists(&self, hash: PayloadHash) -> Result<bool, StoreError> {
        match fs::remove_file(self.path_for(hash)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn read(&self, hash: PayloadHash) -> Result<Vec<u8>, StoreError> {
        fs::read(self.path_for(hash)).map_err(Into::into)
    }

    pub(crate) fn visit_entries(
        &self,
        mut visitor: impl FnMut(PayloadEntry) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        if !self.root.exists() {
            return Ok(());
        }
        for first in fs::read_dir(&self.root)? {
            let first = first?;
            if !first.file_type()?.is_dir() {
                continue;
            }
            for second in fs::read_dir(first.path())? {
                let second = second?;
                if !second.file_type()?.is_dir() {
                    continue;
                }
                for file in fs::read_dir(second.path())? {
                    let file = file?;
                    if !file.file_type()?.is_file() {
                        continue;
                    }
                    let name = file.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with(".stage-") {
                        visitor(PayloadEntry::Staged(file.path()))?;
                    } else if let Some(hash) = parse_hash(&name) {
                        visitor(PayloadEntry::Payload(hash))?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) enum PayloadEntry {
    Payload(PayloadHash),
    Staged(PathBuf),
}

fn parse_hash(value: &str) -> Option<PayloadHash> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16).ok()?;
    }
    Some(PayloadHash(bytes))
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn put_is_content_addressed_and_idempotent() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("clipboard-payload-test-{unique}"));
        let store = PayloadStore::new(&root);
        let first = store.put(b"payload").unwrap();
        let second = store.put(b"payload").unwrap();
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(fs::read(first.path).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }
}
