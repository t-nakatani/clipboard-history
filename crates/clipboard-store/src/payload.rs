use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::StoreError;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Chunk size used when hashing an existing payload for verification.
const VERIFY_CHUNK_BYTES: usize = 64 * 1024;

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
        if verify_file(&destination, hash, bytes.len() as u64)? {
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
            Err(_error) if verify_file(&destination, hash, bytes.len() as u64)? => {
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
        let path = self.path_for(hash);
        // Overwrite the payload before unlinking so the plaintext is gone from the
        // file the moment the last reference disappears, instead of lingering in
        // whatever blocks the filesystem later hands out. This is best effort: on
        // copy-on-write filesystems such as APFS the write may land on new blocks
        // and leave the originals readable until they are reused, and existing
        // snapshots, Time Machine backups and iCloud copies keep their own data.
        // Overwrite failures must not block deletion, so they are ignored.
        let _ = zero_file(&path);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Removes a staged temporary payload, overwriting it first with the same
    /// best-effort guarantees as [`PayloadStore::remove_if_exists`].
    pub(crate) fn remove_staged(&self, path: &Path) -> Result<bool, StoreError> {
        let _ = zero_file(path);
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Reads a payload after validating that the on-disk file is a regular
    /// file, is no larger than `max_bytes`, and hashes to `hash`.
    pub fn read(&self, hash: PayloadHash, max_bytes: u64) -> Result<Vec<u8>, StoreError> {
        read_verified(&self.path_for(hash), hash, max_bytes)
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

/// Opens a payload file without following a symlink at the final component.
///
/// The caller supplies the access mode so that both the read path and the
/// delete-time overwrite share these protections.
fn open_payload_file(path: &Path, options: &mut OpenOptions) -> Result<File, StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW rejects a symlink at the final component; O_NONBLOCK keeps
        // a FIFO planted at the payload path from blocking the open until a
        // writer appears. Regular file reads are unaffected by O_NONBLOCK, and
        // the FIFO is rejected by the regular-file check after the open.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(StoreError::InvalidData("payload path is a symbolic link"));
        }
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            Err(StoreError::InvalidData("payload path is a symbolic link"))
        }
        Err(error) => Err(error.into()),
    }
}

/// Opens a payload file and rejects it unless it is a regular file no larger
/// than `max_bytes`. Returns the opened file and its size.
fn open_validated(path: &Path, max_bytes: u64) -> Result<(File, u64), StoreError> {
    let file = open_payload_file(path, OpenOptions::new().read(true))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(StoreError::InvalidData(
            "payload path is not a regular file",
        ));
    }
    let size = metadata.len();
    if size > max_bytes {
        return Err(StoreError::InvalidData(
            "payload exceeds the configured restore byte limit",
        ));
    }
    Ok((file, size))
}

/// Reads and validates the payload stored at `path`.
///
/// The read is bounded by the size reported by the opened file, which is
/// itself rejected when it exceeds `max_bytes`, so a large file on disk can
/// never cause a large allocation.
fn read_verified(path: &Path, hash: PayloadHash, max_bytes: u64) -> Result<Vec<u8>, StoreError> {
    let (file, size) = open_validated(path, max_bytes)?;
    let mut bytes = Vec::with_capacity(size as usize);
    let mut reader = file.take(size);
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(StoreError::InvalidData(
            "payload changed size while reading",
        ));
    }
    if PayloadHash::of(&bytes) != hash {
        return Err(StoreError::InvalidData(
            "payload failed integrity verification",
        ));
    }
    Ok(bytes)
}

/// Validates the payload stored at `path` without buffering it.
///
/// The file is hashed in fixed-size chunks, so verifying an existing payload
/// never allocates proportionally to its size.
fn verify_stored(path: &Path, hash: PayloadHash, expected_size: u64) -> Result<(), StoreError> {
    let (file, size) = open_validated(path, expected_size)?;
    if size != expected_size {
        return Err(StoreError::InvalidData(
            "payload size does not match the expected size",
        ));
    }

    let mut reader = file.take(size);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; VERIFY_CHUNK_BYTES.min(size.max(1) as usize)];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    if total != size {
        return Err(StoreError::InvalidData(
            "payload changed size while reading",
        ));
    }
    if PayloadHash(*hasher.finalize().as_bytes()) != hash {
        return Err(StoreError::InvalidData(
            "payload failed integrity verification",
        ));
    }
    Ok(())
}

/// Returns `true` when a valid payload for `hash` already exists at `path`.
///
/// A missing file is reported as `false`; a file that fails validation is also
/// reported as `false` so the caller rewrites it from trusted bytes.
fn verify_file(path: &Path, hash: PayloadHash, expected_size: u64) -> Result<bool, StoreError> {
    match verify_stored(path, hash, expected_size) {
        Ok(()) => Ok(true),
        Err(StoreError::InvalidData(_)) => Ok(false),
        Err(StoreError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Overwrites the payload at `path` with zeros.
///
/// Uses the same no-follow open as the read path, so a payload path swapped
/// for a symlink can never redirect the overwrite onto another file, and only
/// regular files are written to.
fn zero_file(path: &Path) -> Result<(), StoreError> {
    const CHUNK: usize = 64 * 1024;
    let mut file = open_payload_file(path, OpenOptions::new().write(true))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(StoreError::InvalidData(
            "payload path is not a regular file",
        ));
    }
    let mut remaining = metadata.len();
    let zeros = [0_u8; CHUNK];
    while remaining > 0 {
        let take = remaining.min(CHUNK as u64) as usize;
        file.write_all(&zeros[..take])?;
        remaining -= take as u64;
    }
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const LIMIT: u64 = 1024 * 1024;

    fn temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("clipboard-payload-{label}-{unique}"))
    }

    fn assert_invalid(error: StoreError) {
        assert!(
            matches!(error, StoreError::InvalidData(_)),
            "expected invalid data error, got {error:?}"
        );
    }

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

    #[test]
    fn read_rejects_replaced_payload_content() {
        let root = temp_root("replaced");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"original").unwrap();
        fs::write(&stored.path, b"attacker").unwrap();

        assert_invalid(store.read(stored.hash, LIMIT).unwrap_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_rejects_hash_mismatch_of_equal_length() {
        let root = temp_root("mismatch");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"payload-a").unwrap();
        fs::write(&stored.path, b"payload-b").unwrap();

        assert_invalid(store.read(stored.hash, LIMIT).unwrap_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_rejects_oversized_file_on_disk() {
        let root = temp_root("oversized");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"small").unwrap();
        // The recorded size stays small while the on-disk file grows.
        fs::write(&stored.path, vec![0_u8; 4096]).unwrap();

        assert_invalid(store.read(stored.hash, 8).unwrap_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn put_does_not_accept_corrupted_existing_payload() {
        let root = temp_root("put-corrupt");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"payload").unwrap();
        fs::write(&stored.path, b"corrupted").unwrap();

        let again = store.put(b"payload").unwrap();
        assert!(again.created);
        assert_eq!(fs::read(&again.path).unwrap(), b"payload");
        assert_eq!(store.read(stored.hash, LIMIT).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn read_rejects_symlink_at_payload_path() {
        let root = temp_root("symlink");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"payload").unwrap();
        let target = root.join("elsewhere");
        fs::write(&target, b"payload").unwrap();
        fs::remove_file(&stored.path).unwrap();
        std::os::unix::fs::symlink(&target, &stored.path).unwrap();

        assert_invalid(store.read(stored.hash, LIMIT).unwrap_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn read_rejects_fifo_at_payload_path_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};

        let root = temp_root("fifo");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"payload").unwrap();
        fs::remove_file(&stored.path).unwrap();
        let raw = CString::new(stored.path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `raw` is a valid NUL-terminated path owned by this test.
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);

        // Without O_NONBLOCK this open would block until a writer appears.
        assert_invalid(store.read(stored.hash, LIMIT).unwrap_err());

        let again = store.put(b"payload").unwrap();
        assert!(again.created);
        assert_eq!(store.read(stored.hash, LIMIT).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn put_replaces_symlink_at_payload_path() {
        let root = temp_root("put-symlink");
        let store = PayloadStore::new(&root);
        let stored = store.put(b"payload").unwrap();
        let target = root.join("elsewhere");
        fs::write(&target, b"attacker").unwrap();
        fs::remove_file(&stored.path).unwrap();
        std::os::unix::fs::symlink(&target, &stored.path).unwrap();

        let again = store.put(b"payload").unwrap();
        assert!(again.created);
        assert!(!fs::symlink_metadata(&again.path).unwrap().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"attacker");
        assert_eq!(store.read(stored.hash, LIMIT).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remove_overwrites_payload_before_unlinking() {
        let root = temp_root("wipe");
        let store = PayloadStore::new(&root);
        let secret = b"correct horse battery staple";
        let stored = store.put(secret).unwrap();
        // A second link to the same inode observes what was written to the file
        // itself, which the unlink alone would leave untouched.
        let witness = root.join("witness");
        fs::hard_link(&stored.path, &witness).unwrap();

        assert!(store.remove_if_exists(stored.hash).unwrap());

        assert!(!stored.path.exists());
        assert_eq!(fs::read(&witness).unwrap(), vec![0_u8; secret.len()]);
        assert!(!store.remove_if_exists(stored.hash).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
