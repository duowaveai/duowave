//! What the updates' tests share: a release as the release workflow makes it — an archive with the program and its
//! release.json, signed with a key of the test's own (never the release key) — a feed served over the loopback, and
//! a station of the test's own: its folders under a temporary root, the program it «runs» and a restart that only
//! notes what the gateway would have become.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::routing::get;
use axum::Router;

use super::install::RELEASE_FILE;
use super::StationEnv;

/// A throwaway signing key: what UPDATE_PUBKEY names in the test's config, and the signatures it makes.
pub struct Signer {
    keys: minisign::KeyPair,
}

impl Default for Signer {
    fn default() -> Signer {
        Signer::new()
    }
}

impl Signer {
    pub fn new() -> Signer {
        Signer { keys: minisign::KeyPair::generate_unencrypted_keypair().unwrap() }
    }

    /// The public key, as UPDATE_PUBKEY takes it.
    pub fn public(&self) -> String {
        self.keys.pk.to_base64()
    }

    /// minisign's signature text of `data`, as `tauri signer sign` makes it.
    pub fn sign(&self, data: &[u8]) -> String {
        minisign::sign(Some(&self.keys.pk), &self.keys.sk, std::io::Cursor::new(data), None, None).unwrap().to_string()
    }
}

/// A program that answers `<program> version` with `version`, as the gateway's own does — a shell script, padded to
/// the size an archive's program must have. On Windows, where a script does not run, `duowave-test-stub` with the
/// version in its last bytes (src/bin/test_stub.rs).
pub fn program_answering(version: &str) -> Vec<u8> {
    if cfg!(windows) {
        let mut program = std::fs::read(crate::utils::test_stub::built()).unwrap();
        program.extend_from_slice(format!("\nDUOWAVE-STUB-VERSION:{version}\n").as_bytes());
        return program;
    }
    let mut script = format!("#!/bin/sh\necho \"duowave {version}\"\n");
    while script.len() < 4096 {
        script.push_str("# padding: a program is at least a kilobyte\n");
    }
    script.into_bytes()
}

/// An archive as the release workflow packs it: the program `duowave` and release.json with its version and platform.
pub fn release_archive(version: &str, platform: &str, program: &[u8]) -> Vec<u8> {
    let release = format!(r#"{{"version":"{version}","platform":"{platform}"}}"#);
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, data) in [("duowave", program), (RELEASE_FILE, release.as_bytes())] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, path, data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// A release site on the loopback: `/latest.json` (what the test sets, changed between checks) and the files put
/// there. Its address, http://127.0.0.1:<port>.
pub struct Site {
    pub base: String,
    pub feed: Arc<Mutex<String>>,
    files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl Site {
    pub async fn start() -> Site {
        let feed = Arc::new(Mutex::new(String::from(r#"{"version":"0.0.1","platforms":{}}"#)));
        let files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>> = Arc::default();
        let (served, stored) = (feed.clone(), files.clone());
        let router =
            Router::new().route("/latest.json", get(move || async move { served.lock().unwrap().clone() })).route(
                "/files/{name}",
                get(move |axum::extract::Path(name): axum::extract::Path<String>| async move {
                    match stored.lock().unwrap().get(&name) {
                        Some(bytes) => (axum::http::StatusCode::OK, bytes.clone()),
                        None => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Site { base: format!("http://{address}"), feed, files }
    }

    pub fn feed_url(&self) -> String {
        format!("{}/latest.json", self.base)
    }

    /// A file on the site, at the address it answers.
    pub fn put(&self, name: &str, bytes: Vec<u8>) -> String {
        self.files.lock().unwrap().insert(name.to_string(), bytes);
        format!("{}/files/{name}", self.base)
    }

    /// The feed of a release `version`: `assets` — each platform's key with its file's address and signature;
    /// `critical_below` — the urgent floor (scripts/release/critical.txt), when the release has one.
    pub fn release(&self, version: &str, critical_below: Option<&str>, assets: &[(&str, &str, &str)]) {
        let platforms: serde_json::Map<String, serde_json::Value> = assets
            .iter()
            .map(|(platform, url, signature)| {
                (platform.to_string(), serde_json::json!({ "url": url, "signature": signature }))
            })
            .collect();
        let mut feed =
            serde_json::json!({ "version": version, "pub_date": "2026-11-20T00:00:00Z", "platforms": platforms });
        if let Some(floor) = critical_below {
            feed["critical_below"] = serde_json::json!(floor);
        }
        *self.feed.lock().unwrap() = feed.to_string();
    }
}

/// A station of the test's own: /data/opt/duowave/bin/duowave (the program it runs, an old version), /tmp for the
/// trial, /proc/meminfo with the memory the test gives it. What the gateway asked to become is noted with what the
/// station's files said at that moment — the restart itself never happens.
pub struct Station {
    pub root: tempfile::TempDir,
    pub became: Arc<Mutex<Vec<Became>>>,
}

/// The moment the gateway would have become another program.
#[derive(Debug, Clone)]
pub struct Became {
    pub program: PathBuf,
    /// The trial's mark as it stood: its version.
    pub trying: Option<String>,
    /// The program on the disk then, its bytes.
    pub on_disk: Vec<u8>,
    /// What the program asked to become answers to `version`.
    pub answers: String,
}

impl Station {
    pub fn new(memory_available_kb: u64) -> Station {
        let root = tempfile::tempdir().unwrap();
        for folder in ["data/opt/duowave/bin", "tmp", "proc/sys/kernel/random"] {
            std::fs::create_dir_all(root.path().join(folder)).unwrap();
        }
        std::fs::write(root.path().join("data/opt/duowave/bin/duowave"), program_answering("0.0.1")).unwrap();
        std::fs::write(
            root.path().join("proc/meminfo"),
            format!("MemTotal: 500000 kB\nMemFree: 1000 kB\nMemAvailable: {memory_available_kb} kB\n"),
        )
        .unwrap();
        std::fs::write(root.path().join("proc/sys/kernel/random/boot_id"), "this-boot\n").unwrap();
        Station { root, became: Arc::default() }
    }

    pub fn program(&self) -> PathBuf {
        self.root.path().join("data/opt/duowave/bin/duowave")
    }

    /// What `UpdatesService::set_station_env` takes: this station, its program running, the restart noted.
    pub fn env(&self) -> StationEnv {
        let (root, became, program) = (self.root.path().to_path_buf(), self.became.clone(), self.program());
        StationEnv {
            root: Some(root.clone()),
            running: Some(program.clone()),
            restart: Some(Arc::new(move |target: &Path| {
                let fs = crate::placement::station_fs::StationFs::at(&root);
                let answers = std::process::Command::new(target)
                    .arg("version")
                    .output()
                    .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                    .unwrap_or_default();
                became.lock().unwrap().push(Became {
                    program: target.to_path_buf(),
                    trying: fs.trial().map(|mark| mark.version),
                    on_disk: std::fs::read(&program).unwrap_or_default(),
                    answers,
                });
                std::io::Error::other("the test's station does not exec")
            })),
        }
    }

    /// Waits (a few seconds at most) until the gateway asked to become another program; what it was.
    pub async fn wait_became(&self) -> Option<Became> {
        for _ in 0..200 {
            if let Some(became) = self.became.lock().unwrap().first().cloned() {
                return Some(became);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        None
    }
}
