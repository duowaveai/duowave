//! The gateway's own Node (docs/plans/feed.md, stage 2): an app of `runtime: node` is a script, and the gateway runs it
//! with a Node of its own — one pinned version, downloaded at the first need and checked by its SHA-256 before it is
//! unpacked, kept in `$DATA_DIR/node/<version>`. A Node already on the computer is never used: its version is unknown
//! and it is not ours to rely on — a person updates or removes it and the app breaks with no reason seen.
//!
//! - **Its own Node** (`DUOWAVE_NODE`): the path to a `node` that serves instead of ours — the Docker image on Alpine,
//!   where Node's own builds do not run (they need glibc), carries its own; a developer may point at theirs.
//! - **Where from** (`DUOWAVE_NODE_MIRROR`): nodejs.org by default; a copy of the same files elsewhere serves as well —
//!   the hash makes any copy safe.
//! - **An app's dependencies**: `npm ci` from its `package-lock.json` into its folder, without the packages' install
//!   scripts (`--ignore-scripts`): only plain JavaScript or packages with their builds ready install for people — a
//!   compiler is not on a person's computer. Once per lock: a mark beside them says which lock they are of.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// The pinned version: Node 24 LTS. macOS 13.5 at least — as the desktop app (tauri.conf.json); Linux with glibc 2.28.
pub const VERSION: &str = "24.22.3";
/// The builds of [`VERSION`] by our platform (`manifest::platform()`): the file's end on the mirror and its SHA-256
/// from nodejs.org's SHASUMS256.txt.
const BUILDS: &[(&str, &str, &str)] = &[
    ("darwin-arm64", "darwin-arm64.tar.gz", "ff6e17d4874b76d272212b908cc020d6e815fba5a6c7eaaa06add725840307fd"),
    ("darwin-x86_64", "darwin-x64.tar.gz", "977614af0504c81045e7fd85596dbca0e5f951b96681516dadafedcca8c447f7"),
    ("linux-aarch64", "linux-arm64.tar.gz", "261af2af250dfd7e9f25cf151d1b2713a0adf8f10a9ad116c4947f68627affe8"),
    ("linux-x86_64", "linux-x64.tar.gz", "061ec3e3062115705b56b51fa9cc330ab50d34b6969387d4e4b1a50c6809acc2"),
    ("windows-x86_64", "win-x64.zip", "e612af99f626ab415ea225af6bce5c971feb448cb5cae250bb1c3fa50c5f7d9f"),
];
/// A Node of one's own instead of ours (config: `own_node`).
pub const OWN_NODE_ENV: &str = "DUOWAVE_NODE";
const MIRROR: &str = "https://nodejs.org/dist";
/// Node's archive is about 50 MB; more than this is not it.
const MAX_ARCHIVE: u64 = 150 * 1024 * 1024;
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(15 * 60);
const DEPENDENCIES_LIMIT: Duration = Duration::from_secs(15 * 60);
/// The mark beside an app's dependencies: the SHA-256 of the lock they are of.
const DEPENDENCIES_MARK: &str = ".duowave-dependencies";

/// A build of Node: the file's end on the mirror and its SHA-256.
#[derive(Debug, Clone)]
pub struct Build {
    pub file: String,
    pub sha256: String,
}

/// A Node ready to run: its program, npm's script beside it, and the folder of its programs (it goes first in PATH).
#[derive(Debug, Clone)]
pub struct Node {
    pub program: PathBuf,
    pub npm: Option<PathBuf>,
    pub bin_dir: PathBuf,
}

/// Who hears what is being done, in words: the app's journal.
pub type Tell<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// The gateway's Node: where it lives, where it comes from.
pub struct NodeHome {
    dir: PathBuf,
    mirror: String,
    own: Option<PathBuf>,
    build: Option<Build>,
    version: String,
    /// One download, one unpacking at a time.
    turn: tokio::sync::Mutex<()>,
}

impl NodeHome {
    /// In `dir`, with a Node of one's own if given, from a mirror if given.
    pub fn new(dir: PathBuf, own: Option<PathBuf>, mirror: Option<String>) -> NodeHome {
        let mirror = mirror
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| MIRROR.to_string());
        let platform = crate::plugins::manifest::platform();
        let build = BUILDS
            .iter()
            .find(|(key, _, _)| *key == platform)
            .map(|(_, file, sha256)| Build { file: file.to_string(), sha256: sha256.to_string() });
        NodeHome::with(dir, own, mirror, build, VERSION.to_string())
    }

    pub fn with(dir: PathBuf, own: Option<PathBuf>, mirror: String, build: Option<Build>, version: String) -> NodeHome {
        NodeHome { dir, mirror, own, build, version, turn: tokio::sync::Mutex::new(()) }
    }

    /// Where our Node of the pinned version lies once unpacked.
    fn ours(&self) -> PathBuf {
        self.dir.join(&self.version)
    }

    /// The Node to run an app with: the own one if set, else ours — downloaded and unpacked now if it is not here.
    pub async fn node(&self, tell: Tell<'_>) -> Result<Node, String> {
        if let Some(own) = &self.own {
            if !own.is_file() {
                return Err(format!("{OWN_NODE_ENV}={}: такого файла нет", own.display()));
            }
            return Ok(beside(own));
        }
        let _turn = self.turn.lock().await;
        let home = self.ours();
        if let Some(node) = placed(&home) {
            return Ok(node);
        }
        let build = self.build.clone().ok_or_else(|| {
            format!(
                "для этой системы ({}) готового Node нет: укажите свой в {OWN_NODE_ENV}",
                crate::plugins::manifest::platform()
            )
        })?;
        let name = format!("node-v{}-{}", self.version, build.file);
        let url = format!("{}/v{}/{name}", self.mirror, self.version);
        tell(&format!("Скачиваю Node {} — около 50 МБ, один раз для всех приложений на Node", self.version));
        std::fs::create_dir_all(&self.dir).map_err(|error| format!("{}: {error}", self.dir.display()))?;
        let archive = self.dir.join(format!(".download-{name}"));
        let downloaded = tokio::time::timeout(DOWNLOAD_LIMIT, download(&url, &archive)).await;
        let sha256 = match downloaded {
            Ok(Ok(sha256)) => sha256,
            Ok(Err(error)) => {
                let _ = std::fs::remove_file(&archive);
                return Err(format!("Node не скачался: {error}"));
            }
            Err(_) => {
                let _ = std::fs::remove_file(&archive);
                return Err(format!("Node не скачался за {} мин", DOWNLOAD_LIMIT.as_secs() / 60));
            }
        };
        if !sha256.eq_ignore_ascii_case(&build.sha256) {
            let _ = std::fs::remove_file(&archive);
            return Err(format!("{name}: хеш не совпал — файл подменён или испорчен, Node не поставлен"));
        }
        tell(&format!("Распаковываю Node {}", self.version));
        let (dir, version, zip) = (self.dir.clone(), self.version.clone(), build.file.ends_with(".zip"));
        let archive_at = archive.clone();
        let unpacked = tokio::task::spawn_blocking(move || unpack(&archive_at, &dir, &version, zip))
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        let _ = std::fs::remove_file(&archive);
        unpacked?;
        let node = placed(&home).ok_or_else(|| format!("в архиве {name} нет программы node"))?;
        tell(&format!("Node {} на месте", self.version));
        Ok(node)
    }

    /// An app's dependencies, from its `package-lock.json`, put beside it unless the ones there are of this lock.
    /// Without a lock there is nothing to put: what the package brought is what it runs with.
    pub async fn dependencies(&self, node: &Node, app: &Path, tell: Tell<'_>) -> Result<(), String> {
        let lock = app.join("package-lock.json");
        let Ok(bytes) = std::fs::read(&lock) else { return Ok(()) };
        let of_lock = hex::encode(Sha256::digest(&bytes));
        let mark = app.join("node_modules").join(DEPENDENCIES_MARK);
        if std::fs::read_to_string(&mark).is_ok_and(|text| text.trim() == of_lock) {
            return Ok(());
        }
        let npm = node.npm.as_ref().ok_or("рядом с Node нет npm — зависимости приложения не поставить")?;
        tell("Ставлю зависимости приложения из package-lock.json (npm ci, без их сценариев установки)");
        let home = self.dir.join("home");
        let cache = self.dir.join("npm-cache");
        for folder in [&home, &cache] {
            std::fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        }
        let mut command = tokio::process::Command::new(&node.program);
        command
            .arg(npm)
            .args([
                "ci",
                "--ignore-scripts",
                "--omit=dev",
                "--no-audit",
                "--no-fund",
                "--no-update-notifier",
                "--loglevel=error",
            ])
            .current_dir(app)
            .env_clear()
            .env("PATH", path_with(&node.bin_dir))
            .env("HOME", &home)
            .env("npm_config_cache", &cache)
            .env("npm_config_userconfig", self.dir.join("npmrc"))
            .env("npm_config_globalconfig", self.dir.join("npmrc-global"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        keep_system_env(&mut command);
        let output = tokio::time::timeout(DEPENDENCIES_LIMIT, command.output())
            .await
            .map_err(|_| format!("npm ci не закончил за {} мин", DEPENDENCIES_LIMIT.as_secs() / 60))?
            .map_err(|error| format!("npm не запустился: {error}"))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            let last = said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("");
            return Err(format!(
                "npm ci не прошёл ({}): {}",
                output.status,
                last.chars().take(300).collect::<String>()
            ));
        }
        std::fs::create_dir_all(app.join("node_modules")).map_err(|error| error.to_string())?;
        std::fs::write(&mark, &of_lock).map_err(|error| format!("{}: {error}", mark.display()))?;
        tell("Зависимости приложения на месте");
        Ok(())
    }
}

/// PATH with Node's own folder first: what the app starts by name (`node`, `npx`) is our Node.
pub fn path_with(bin_dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![bin_dir.to_path_buf()];
    if let Some(system) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&system));
    }
    std::env::join_paths(paths).unwrap_or_default()
}

/// What a program on Windows cannot start without, kept past `env_clear`.
pub fn keep_system_env(command: &mut tokio::process::Command) {
    if cfg!(windows) {
        for name in
            ["SYSTEMROOT", "SystemRoot", "WINDIR", "TEMP", "TMP", "COMSPEC", "PATHEXT", "APPDATA", "LOCALAPPDATA"]
        {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
}

/// A Node at a path of one's own, npm found beside it as Node's builds and the systems' packages put it.
fn beside(program: &Path) -> Node {
    let bin_dir = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let candidates = [
        bin_dir.join("../lib/node_modules/npm/bin/npm-cli.js"),
        bin_dir.join("node_modules/npm/bin/npm-cli.js"),
        PathBuf::from("/usr/lib/node_modules/npm/bin/npm-cli.js"),
        PathBuf::from("/usr/share/nodejs/npm/bin/npm-cli.js"),
    ];
    let npm = candidates.into_iter().find(|path| path.is_file());
    Node { program: program.to_path_buf(), npm, bin_dir }
}

/// Our Node unpacked in `home`, if it is whole.
/// Whether an app on Node would download our Node first (about 50 MB, once): no Node of one's own (DUOWAVE_NODE), and
/// ours of the pinned version not unpacked in `dir` yet. MagicPlace says it before the install.
pub fn needs_download(dir: &Path, own: Option<&Path>) -> bool {
    own.is_none() && placed(&dir.join(VERSION)).is_none()
}

fn placed(home: &Path) -> Option<Node> {
    let (program, npm, bin_dir) = if cfg!(windows) {
        (home.join("node.exe"), home.join("node_modules/npm/bin/npm-cli.js"), home.to_path_buf())
    } else {
        (home.join("bin/node"), home.join("lib/node_modules/npm/bin/npm-cli.js"), home.join("bin"))
    };
    program.is_file().then(|| Node { program, npm: npm.is_file().then_some(npm), bin_dir })
}

/// The archive to `to`, counted and hashed on the way: its SHA-256.
async fn download(url: &str, to: &Path) -> Result<String, String> {
    let answer = crate::utils::http::client().get(url).send().await.map_err(|error| error.without_url().to_string())?;
    if !answer.status().is_success() {
        return Err(format!("HTTP {} — {url}", answer.status().as_u16()));
    }
    if answer.content_length().is_some_and(|length| length > MAX_ARCHIVE) {
        return Err(format!("архив больше {} МБ", MAX_ARCHIVE / 1024 / 1024));
    }
    let mut file = tokio::fs::File::create(to).await.map_err(|error| format!("{}: {error}", to.display()))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut stream = answer.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.without_url().to_string())?;
        total += chunk.len() as u64;
        if total > MAX_ARCHIVE {
            return Err(format!("архив больше {} МБ", MAX_ARCHIVE / 1024 / 1024));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|error| error.to_string())?;
    }
    file.flush().await.map_err(|error| error.to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

/// Node's archive into `<dir>/<version>`: unpacked aside, its one folder moved into place whole; Node of other
/// versions leaves.
fn unpack(archive: &Path, dir: &Path, version: &str, zip: bool) -> Result<(), String> {
    let aside = dir.join(format!(".unpack-{version}"));
    let _ = std::fs::remove_dir_all(&aside);
    std::fs::create_dir_all(&aside).map_err(|error| format!("{}: {error}", aside.display()))?;
    let result = (|| {
        if zip {
            // Windows 10 and later carry bsdtar: it unpacks a zip.
            let status = std::process::Command::new("tar")
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(&aside)
                .status()
                .map_err(|error| format!("tar не запустился: {error}"))?;
            if !status.success() {
                return Err(format!("архив не распаковался ({status})"));
            }
        } else {
            let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
            let mut unpacker = tar::Archive::new(flate2::read::GzDecoder::new(file));
            unpacker.unpack(&aside).map_err(|error| format!("архив не распаковался: {error}"))?;
        }
        let inner = std::fs::read_dir(&aside)
            .map_err(|error| error.to_string())?
            .flatten()
            .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .ok_or("в архиве нет папки Node")?;
        let home = dir.join(version);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::rename(inner.path(), &home).map_err(|error| format!("{}: {error}", home.display()))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&aside);
    result?;
    // Node of an older pin is not used any more.
    for entry in std::fs::read_dir(dir).map_err(|error| error.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != version && name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn archive(version: &str, file: &str) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let top = format!("node-v{version}-{}", file.trim_end_matches(".tar.gz"));
        for (path, body, mode) in [
            (format!("{top}/bin/node"), "#!/bin/sh\necho v{version}\n", 0o755),
            (format!("{top}/lib/node_modules/npm/bin/npm-cli.js"), "// npm\n", 0o644),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(mode);
            header.set_cksum();
            builder.append_data(&mut header, path, body.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// A mirror on this machine that gives `bytes` at any path and counts the asks.
    async fn mirror(bytes: Vec<u8>) -> (String, Arc<Mutex<u32>>) {
        let asked = Arc::new(Mutex::new(0));
        let counted = asked.clone();
        let app = axum::Router::new().fallback(move || {
            let (bytes, counted) = (bytes.clone(), counted.clone());
            async move {
                *counted.lock().unwrap() += 1;
                bytes
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), asked)
    }

    fn quiet(_: &str) {}

    #[tokio::test]
    async fn downloads_our_node_once_checks_its_hash_and_drops_an_older_pin() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive("24.0.0", "test-x64.tar.gz");
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let (url, asked) = mirror(bytes).await;
        std::fs::create_dir_all(dir.path().join("22.1.0/bin")).unwrap();
        let build = Build { file: "test-x64.tar.gz".into(), sha256 };
        let home = NodeHome::with(dir.path().into(), None, url.clone(), Some(build), "24.0.0".into());
        let told = Mutex::new(Vec::<String>::new());
        let tell = |line: &str| told.lock().unwrap().push(line.to_string());
        let node = home.node(&tell).await.unwrap();
        if !cfg!(windows) {
            assert!(node.program.ends_with("24.0.0/bin/node"), "{node:?}");
            assert!(node.npm.as_ref().is_some_and(|npm| npm.is_file()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&node.program).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "node runs");
            }
        }
        assert!(!dir.path().join("22.1.0").exists(), "the older pin left");
        assert!(told.lock().unwrap()[0].starts_with("Скачиваю Node 24.0.0"));
        home.node(&quiet).await.unwrap();
        assert_eq!(*asked.lock().unwrap(), 1, "downloaded once");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // Another hash: nothing is placed.
        let other = tempfile::tempdir().unwrap();
        let wrong = Build { file: "test-x64.tar.gz".into(), sha256: "0".repeat(64) };
        let home = NodeHome::with(other.path().into(), None, url, Some(wrong), "24.0.0".into());
        let error = home.node(&quiet).await.unwrap_err();
        assert!(error.contains("хеш не совпал"), "{error}");
        assert!(!other.path().join("24.0.0").exists());
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0, "the download left too");

        // No build for the system and no own Node: said so.
        let home = NodeHome::with(other.path().into(), None, String::new(), None, "24.0.0".into());
        assert!(home.node(&quiet).await.unwrap_err().contains(OWN_NODE_ENV));
        // An own Node that is not there.
        let home = NodeHome::with(other.path().into(), Some("/no/such/node".into()), String::new(), None, "1".into());
        assert!(home.node(&quiet).await.unwrap_err().contains("такого файла нет"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn puts_an_apps_dependencies_once_per_lock_without_their_scripts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // A «node» that writes down how it was asked and puts a dependency, as npm ci would.
        let program = bin.join("node");
        let said = dir.path().join("said.txt");
        std::fs::write(
            &program,
            format!("#!/bin/sh\necho \"$@\" >> {}\nmkdir -p node_modules/left-pad\n", said.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let npm = dir.path().join("npm-cli.js");
        std::fs::write(&npm, "").unwrap();
        let node = Node { program, npm: Some(npm), bin_dir: bin };
        let home = NodeHome::with(dir.path().join("home"), None, String::new(), None, "1".into());
        let app = dir.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        // No lock: nothing to do.
        home.dependencies(&node, &app, &quiet).await.unwrap();
        assert!(!said.exists());
        std::fs::write(app.join("package-lock.json"), "{\"lockfileVersion\":3}").unwrap();
        home.dependencies(&node, &app, &quiet).await.unwrap();
        home.dependencies(&node, &app, &quiet).await.unwrap();
        let asked = std::fs::read_to_string(&said).unwrap();
        assert_eq!(asked.lines().count(), 1, "once per lock: {asked}");
        assert!(asked.contains(" ci --ignore-scripts --omit=dev"), "{asked}");
        assert!(app.join("node_modules/left-pad").is_dir());
        // A new lock: again.
        std::fs::write(app.join("package-lock.json"), "{\"lockfileVersion\":3,\"x\":1}").unwrap();
        home.dependencies(&node, &app, &quiet).await.unwrap();
        assert_eq!(std::fs::read_to_string(&said).unwrap().lines().count(), 2);
        // No npm: said so.
        let bare = Node { npm: None, ..node.clone() };
        std::fs::write(app.join("package-lock.json"), "{}").unwrap();
        assert!(home.dependencies(&bare, &app, &quiet).await.unwrap_err().contains("нет npm"));
    }
}
