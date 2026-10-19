//! Attachment names and files (sections 8 and 13.5): the name shown and saved, the
//! confirmation rule for executable types, the on-disk byte cache (LRU, 500 MB) and the
//! temp folders for Open.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mail_parser::{MessageParser, MimeHeaders};

/// The attachment cache's default cap (invariant 3).
pub const CACHE_CAP: u64 = 500 * 1024 * 1024;

/// Longest saved name, in characters (section 8).
const MAX_NAME: usize = 200;

/// Extensions that need a confirmation before Open (section 8).
const EXECUTABLE: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "js", "jar", "lnk", "app",
];

/// Windows reserved device names (section 8).
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The fallback when a part has no usable name.
pub const FALLBACK_NAME: &str = "attachment";

/// The name as the sender meant it: RFC 2047 encoded words decoded (RFC 2231 values are
/// decoded when BODYSTRUCTURE is read). Never fails.
pub fn display_name(raw: &str) -> String {
    let raw = raw.trim();
    if !raw.contains("=?") {
        return raw.to_string();
    }
    let header = format!(
        "Content-Type: application/octet-stream; name=\"{}\"\r\n\r\n",
        raw.replace(['"', '\r', '\n'], "")
    );
    MessageParser::default()
        .parse_headers(header.as_bytes())
        .and_then(|m| {
            m.content_type()
                .and_then(|ct| ct.attribute("name"))
                .map(str::to_string)
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| raw.to_string())
}

/// A name safe to create on any OS (section 8, SPEC_GAPS G55): path separators, control
/// characters and `: * ? " < > |` stripped, Windows reserved names prefixed with `_`, at most
/// 200 characters, and [`FALLBACK_NAME`] when nothing usable is left.
pub fn safe_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect();
    cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        return FALLBACK_NAME.to_string();
    }
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        cleaned.insert(0, '_');
    }
    if cleaned.chars().count() > MAX_NAME {
        // Keep the extension when it is short.
        let ext: String = match cleaned.rsplit_once('.') {
            Some((_, e)) if e.chars().count() <= 16 => format!(".{e}"),
            _ => String::new(),
        };
        let keep = MAX_NAME - ext.chars().count();
        cleaned = cleaned.chars().take(keep).collect::<String>() + &ext;
    }
    cleaned
}

/// Whether Open must ask first (section 8): the extension is an executable type.
pub fn is_executable(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.trim().to_ascii_lowercase())
        .is_some_and(|ext| EXECUTABLE.contains(&ext.as_str()))
}

/// The on-disk cache of attachment bytes (invariant 3): one file per part, evicted least
/// recently used first when the total passes the cap. Raw message source is never cached.
#[derive(Debug, Clone)]
pub struct AttachmentCache {
    dir: PathBuf,
    cap: u64,
}

/// What identifies one part's bytes across purges and row-id reuse: the account, folder,
/// its UIDVALIDITY, the UID and the part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    pub account_id: i64,
    pub folder_id: i64,
    pub uidvalidity: u32,
    pub uid: u32,
    pub part_id: String,
}

impl CacheKey {
    fn file_name(&self) -> String {
        let part: String = self
            .part_id
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        format!(
            "{}-{}-{}-{}-{part}",
            self.account_id, self.folder_id, self.uidvalidity, self.uid
        )
    }
}

impl AttachmentCache {
    pub fn new(dir: PathBuf, cap: u64) -> Self {
        Self { dir, cap }
    }

    /// Cached bytes, marking them recently used.
    pub fn get(&self, key: &CacheKey) -> Option<Vec<u8>> {
        let path = self.dir.join(key.file_name());
        let bytes = std::fs::read(&path).ok()?;
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    /// Stores bytes, then evicts the least recently used files over the cap. A part larger
    /// than the cap is not cached.
    pub fn put(&self, key: &CacheKey, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.len() as u64 > self.cap {
            return Ok(());
        }
        create_private_dir(&self.dir)?;
        let path = self.dir.join(key.file_name());
        let partial = self.dir.join(format!("{}.partial", key.file_name()));
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &path)?;
        self.evict(&path)
    }

    fn evict(&self, keep: &Path) -> std::io::Result<()> {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = std::fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                meta.is_file().then(|| {
                    (
                        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        meta.len(),
                        e.path(),
                    )
                })
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        files.sort_by_key(|(at, _, _)| *at);
        for (_, len, path) in files {
            if total <= self.cap {
                break;
            }
            if path != keep {
                std::fs::remove_file(&path)?;
                total = total.saturating_sub(len);
            }
        }
        Ok(())
    }

    /// Drops one account's cached files (account removal, section 5).
    pub fn remove_account(&self, account_id: i64) -> std::io::Result<()> {
        let prefix = format!("{account_id}-");
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Ok(());
        };
        for e in entries.filter_map(Result::ok) {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                std::fs::remove_file(e.path())?;
            }
        }
        Ok(())
    }
}

/// Temp folders for attachments the user opens (section 13.5): each file in a randomly named
/// subfolder with user-only permissions; everything is deleted at quit and at the next start.
#[derive(Debug, Clone)]
pub struct TempFiles {
    root: PathBuf,
}

impl TempFiles {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Writes `bytes` as `<root>/<random>/<name>` and returns the path.
    pub fn write(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        create_private_dir(&self.root)?;
        let dir = self.root.join(crate::nonce());
        create_private_dir(&dir)?;
        let path = dir.join(safe_name(name));
        std::fs::write(&path, bytes)?;
        Ok(path)
    }

    /// Deletes every temp folder.
    pub fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.root) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// A directory only the user can read (section 13.9). On Windows the app-data directory is
/// already per-user.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(safe_name("../x"), "..x");
        assert_eq!(safe_name("..\\..\\boot.ini"), "....boot.ini");
        assert_eq!(safe_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_name("con"), "_con");
        assert_eq!(safe_name("lpt9.tar.gz"), "_lpt9.tar.gz");
        assert_eq!(safe_name("console.txt"), "console.txt");
        assert_eq!(safe_name("a:b*c?d\"e<f>g|h\u{0}\u{7}.txt"), "abcdefgh.txt");
        assert_eq!(safe_name(".."), FALLBACK_NAME);
        assert_eq!(safe_name("  / "), FALLBACK_NAME);
        let long = format!("{}.pdf", "é".repeat(300));
        let cut = safe_name(&long);
        assert_eq!(cut.chars().count(), 200);
        assert!(cut.ends_with(".pdf"));
    }

    #[test]
    fn executable_types_need_confirmation() {
        for name in ["setup.EXE", "x.ps1", "a.b.lnk", "run.js", "My.app"] {
            assert!(is_executable(name), "{name}");
        }
        for name in ["report.pdf", "exe", "photo.jpeg", "archive.json"] {
            assert!(!is_executable(name), "{name}");
        }
    }

    #[test]
    fn encoded_names_are_decoded() {
        assert_eq!(display_name("=?UTF-8?B?Q2Fmw6kucGRm?="), "Café.pdf");
        assert_eq!(
            display_name("=?iso-8859-1?Q?r=E9sum=E9.doc?="),
            "résumé.doc"
        );
        assert_eq!(display_name("plain.txt"), "plain.txt");
    }

    fn key(account_id: i64, uid: u32) -> CacheKey {
        CacheKey {
            account_id,
            folder_id: 2,
            uidvalidity: 3,
            uid,
            part_id: "1.2".into(),
        }
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let dir = std::env::temp_dir().join(format!("zm-cache-{}", crate::nonce()));
        let cache = AttachmentCache::new(dir.clone(), 25);
        cache.put(&key(1, 1), &[1; 10]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        cache.put(&key(1, 2), &[2; 10]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        // Using the first makes the second the oldest.
        assert_eq!(cache.get(&key(1, 1)), Some(vec![1; 10]));
        std::thread::sleep(std::time::Duration::from_millis(30));
        cache.put(&key(2, 3), &[3; 10]).unwrap();
        assert!(cache.get(&key(1, 2)).is_none(), "evicted");
        assert!(cache.get(&key(1, 1)).is_some() && cache.get(&key(2, 3)).is_some());
        cache.put(&key(1, 4), &[4; 30]).unwrap();
        assert!(
            cache.get(&key(1, 4)).is_none(),
            "larger than the cap: not cached"
        );
        cache.remove_account(1).unwrap();
        assert!(cache.get(&key(1, 1)).is_none() && cache.get(&key(2, 3)).is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn temp_files_live_in_random_folders() {
        let root = std::env::temp_dir().join(format!("zm-tmp-{}", crate::nonce()));
        let temp = TempFiles::new(root.clone());
        let a = temp.write("../CON.txt", b"a").unwrap();
        let b = temp.write("../CON.txt", b"b").unwrap();
        assert_ne!(a.parent(), b.parent());
        assert_eq!(a.file_name().unwrap(), "..CON.txt");
        assert!(a.starts_with(&root));
        temp.clear().unwrap();
        assert!(!root.exists());
        temp.clear().unwrap();
    }
}
