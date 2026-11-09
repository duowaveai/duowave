//! Small helpers every module uses: time stamps, ids, the outgoing HTTP client.

pub mod http;

pub mod time {
    use chrono::{SecondsFormat, Utc};

    /// Now as JavaScript's toISOString wrote it: `2026-11-06T02:11:00.331Z` (the database's and the API's format).
    pub fn now_iso() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    /// A time as the API writes it.
    pub fn iso(at: chrono::DateTime<Utc>) -> String {
        at.to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    /// Milliseconds since the epoch, as Date.now().
    pub fn now_ms() -> u64 {
        Utc::now().timestamp_millis().max(0) as u64
    }
}

pub mod ids {
    /// A random id: a UUID v4, as crypto.randomUUID() gave.
    pub fn uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// `n` random bytes.
    pub fn random_bytes(n: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; n];
        rand::fill(&mut bytes[..]);
        bytes
    }

    /// `n` random bytes as hex.
    pub fn random_hex(n: usize) -> String {
        hex::encode(random_bytes(n))
    }
}

/// Permissions for files that hold people's data: the owner only (no-op where there are no Unix modes).
pub mod files {
    use std::path::Path;

    pub fn owner_only(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(path) {
                let mode = if meta.is_dir() { 0o700 } else { 0o600 };
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
            }
        }
        #[cfg(not(unix))]
        let _ = path;
    }
}
