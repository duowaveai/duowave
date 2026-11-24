//! Sealing secrets for the database: AES-256-GCM with a key that lives in its own file beside the database, so the
//! database and its copies (backups before migrations, a file handed over for debugging) hold only ciphertext. Each
//! value is bound to its id: a sealed value moved to another row does not open.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use crate::utils::files::owner_only;
use crate::utils::ids::random_bytes;

/// A sealed value as stored: base64 parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub ciphertext: String,
    pub iv: String,
    pub tag: String,
}

pub const KEY_BYTES: usize = 32;
const TAG_BYTES: usize = 16;

pub type Key = [u8; KEY_BYTES];

pub fn seal(key: &Key, id: &str, value: &str) -> Sealed {
    let cipher = Aes256Gcm::new_from_slice(key).expect("a 32-byte key");
    let iv = random_bytes(12);
    let nonce = Nonce::try_from(&iv[..]).expect("a 12-byte nonce");
    let mut sealed =
        cipher.encrypt(&nonce, Payload { msg: value.as_bytes(), aad: id.as_bytes() }).expect("AES-GCM seals any value");
    let tag = sealed.split_off(sealed.len() - TAG_BYTES);
    Sealed { ciphertext: B64.encode(&sealed), iv: B64.encode(&iv), tag: B64.encode(&tag) }
}

/// The value, or None when the key or the id does not fit (another key file, a row moved by hand).
pub fn open(key: &Key, id: &str, sealed: &Sealed) -> Option<String> {
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let iv = B64.decode(&sealed.iv).ok()?;
    let nonce = Nonce::try_from(&iv[..]).ok()?;
    let mut message = B64.decode(&sealed.ciphertext).ok()?;
    message.extend(B64.decode(&sealed.tag).ok()?);
    let plain = cipher.decrypt(&nonce, Payload { msg: &message, aad: id.as_bytes() }).ok()?;
    String::from_utf8(plain).ok()
}

pub struct LoadedKey {
    pub key: Key,
    pub created: bool,
    pub file: Option<PathBuf>,
}

/// The key from `<dir>/secrets.key`, made on first use (readable by the owner only). Without a directory (a
/// database in memory) the key lives in memory and dies with the process.
pub fn load_key(dir: Option<&Path>) -> std::io::Result<LoadedKey> {
    let fresh = || -> Key {
        let mut key = [0u8; KEY_BYTES];
        key.copy_from_slice(&random_bytes(KEY_BYTES));
        key
    };
    let Some(dir) = dir else {
        return Ok(LoadedKey { key: fresh(), created: true, file: None });
    };
    let file = dir.join("secrets.key");
    if file.exists() {
        let text = std::fs::read_to_string(&file)?;
        if let Ok(bytes) = B64.decode(text.trim()) {
            if bytes.len() == KEY_BYTES {
                owner_only(&file);
                let mut key = [0u8; KEY_BYTES];
                key.copy_from_slice(&bytes);
                return Ok(LoadedKey { key, created: false, file: Some(file) });
            }
        }
        // Not a key (edited by hand?): kept aside, never overwritten.
        let aside = dir.join(format!("secrets.key.broken-{}", crate::utils::time::now_ms()));
        std::fs::rename(&file, aside)?;
    }
    std::fs::create_dir_all(dir)?;
    let key = fresh();
    std::fs::write(&file, format!("{}\n", B64.encode(key)))?;
    owner_only(&file);
    Ok(LoadedKey { key, created: true, file: Some(file) })
}

/// «…a1b2»: enough to tell two keys apart, never enough to use one.
pub fn hint(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() > 8 {
        format!("…{}", chars[chars.len() - 4..].iter().collect::<String>())
    } else {
        "…".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_bound_to_the_id() {
        let key = load_key(None).unwrap().key;
        let sealed = seal(&key, "llm:claude:key", "sk-ant-secret");
        assert_eq!(open(&key, "llm:claude:key", &sealed).as_deref(), Some("sk-ant-secret"));
        assert_eq!(open(&key, "llm:other:key", &sealed), None, "moved to another row");
        let other = load_key(None).unwrap().key;
        assert_eq!(open(&other, "llm:claude:key", &sealed), None, "another key");
    }

    #[test]
    fn keeps_the_key_in_its_file_and_sets_a_broken_one_aside() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_key(Some(dir.path())).unwrap();
        assert!(first.created);
        let again = load_key(Some(dir.path())).unwrap();
        assert!(!again.created);
        assert_eq!(first.key, again.key);
        std::fs::write(dir.path().join("secrets.key"), "not a key").unwrap();
        let fresh = load_key(Some(dir.path())).unwrap();
        assert!(fresh.created);
        assert!(std::fs::read_dir(dir.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("secrets.key.broken-")));
    }

    #[test]
    fn hints_only_the_end() {
        assert_eq!(hint("sk-ant-api03-abcd1234"), "…1234");
        assert_eq!(hint("short"), "…");
    }
}
