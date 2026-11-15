//! What the program keeps in its own folder (`DUOWAVE_APP_DATA`), readable by its owner only:
//!
//! - `connections.json` — the connections as people pasted them: `[{id, name, source}]`. Here, not among the app's
//!   settings: the settings form is one line and gives a secret back to the window masked, so the window could not add
//!   a second connection without erasing the first, and a WireGuard file has several lines.
//! - `state.json` — what the program learned: the person's choice per connection, its entrance's port (the address
//!   stays the same across restarts), the last good subscription (it works on it while the subscription is down), the
//!   servers' countries from Cloudflare's trace.
//!
//! Both are written whole, through a temporary file and a rename.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// The decoded body of the last good reading.
    #[serde(default)]
    pub body: String,
    /// Its `profile-title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// When it was read, seconds since the epoch.
    #[serde(default)]
    pub at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Learned {
    /// `auto` or a server's name.
    #[serde(default = "auto")]
    pub choice: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<Subscription>,
    /// Server name → its country, found by the trace.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub countries: BTreeMap<String, String>,
}

fn auto() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    pub clash_port: u16,
    #[serde(default)]
    pub connections: BTreeMap<String, Learned>,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        let _ = std::fs::create_dir_all(dir);
        Store { dir: dir.to_path_buf() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn read<T: for<'de> Deserialize<'de> + Default>(&self, name: &str) -> T {
        match std::fs::read(self.dir.join(name)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("own-server: {name} does not read ({error}); starting empty");
                T::default()
            }),
            Err(_) => T::default(),
        }
    }

    pub fn connections(&self) -> Vec<Connection> {
        self.read("connections.json")
    }

    pub fn state(&self) -> State {
        self.read("state.json")
    }

    pub fn save_connections(&self, connections: &[Connection]) -> std::io::Result<()> {
        write_private(&self.dir.join("connections.json"), &serde_json::to_vec_pretty(connections)?)
    }

    pub fn save_state(&self, state: &State) -> std::io::Result<()> {
        write_private(&self.dir.join("state.json"), &serde_json::to_vec_pretty(state)?)
    }
}

/// The file replaced at once, readable by its owner only (it holds passwords).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

/// A short random id: `c` and six letters or digits.
pub fn new_id(taken: &[Connection]) -> String {
    loop {
        let bytes = random_bytes(6);
        let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let id: String =
            std::iter::once('c').chain(bytes.iter().map(|b| alphabet[*b as usize % alphabet.len()] as char)).collect();
        if !taken.iter().any(|one| one.id == id) {
            return id;
        }
    }
}

/// A random secret for a local door (the entrances' login, the Clash API).
pub fn secret() -> String {
    random_bytes(18).iter().map(|b| format!("{b:02x}")).collect()
}

fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut bytes = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes)).is_err() {
        // No /dev/urandom: the time's nanoseconds, enough for an id, and the doors are on this machine only.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = (nanos >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(31);
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_private_and_read_back() {
        let dir = std::env::temp_dir().join(format!("own-server-store-{}", secret()));
        let store = Store::new(&dir);
        assert!(store.connections().is_empty());
        let one = Connection { id: new_id(&[]), name: "Мой VPN".into(), source: "vless://x@h:1".into() };
        store.save_connections(std::slice::from_ref(&one)).unwrap();
        assert_eq!(store.connections(), vec![one.clone()]);
        let mut state = State { clash_port: 9, ..State::default() };
        state.connections.insert(one.id.clone(), Learned { choice: "auto".into(), port: 7, ..Learned::default() });
        store.save_state(&state).unwrap();
        assert_eq!(store.state(), state);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("connections.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(one.id.starts_with('c') && one.id.len() == 7);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
