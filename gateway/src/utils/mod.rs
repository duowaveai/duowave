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

/// Programs this gateway started.
pub mod process {
    /// Windows: the program and every program it started, at once (`taskkill /T /F`) — Windows has no signals and no
    /// process groups to send them to; a program killed alone leaves its children running (Claude Code's commands,
    /// uv's Python), holding files and ports. It returns at once; taskkill is waited for on a thread of its own.
    #[cfg(windows)]
    pub fn kill_tree(pid: u32) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut kill = std::process::Command::new("taskkill");
        kill.args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        if let Ok(mut child) = kill.spawn() {
            std::thread::spawn(move || child.wait());
        }
    }
}

/// Programs a test stands in for: `duowave-test-stub` (src/bin/test_stub.rs), built beside the test's own program
/// (`target/<profile>/`, the test being in `target/<profile>/deps/`).
#[cfg(test)]
pub mod test_stub {
    use std::path::{Path, PathBuf};

    /// The stub put at `dir/<role>` (`.exe` on Windows) — its name is its role: `claude`, `acp-stub`, `python`. The
    /// path to it.
    pub fn put(dir: &Path, role: &str) -> PathBuf {
        put_at(&dir.join(format!("{role}{}", std::env::consts::EXE_SUFFIX)))
    }

    /// The stub put at exactly this path — its file name is its role. On Windows a path without `.exe` gets a copy
    /// with it too: Windows runs `python.exe` for `…/python`, while the gateway looks for `…/python` itself (the voice
    /// lab pretends a Mac, where the engine's Python is env/bin/python).
    pub fn put_at(program: &Path) -> PathBuf {
        let built = built();
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::copy(&built, program).unwrap();
        if cfg!(windows) && program.extension().is_none() {
            std::fs::copy(&built, program.with_extension("exe")).unwrap();
        }
        program.to_path_buf()
    }

    /// The stub as `role`, once per test program, beside the build (`<profile>/duowave-test-stub-roles/`): for a test
    /// that only needs a program to run, not a folder of its own around it.
    pub fn role(role: &str) -> PathBuf {
        static PUT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let dir = built().parent().unwrap().join("duowave-test-stub-roles");
        let program = dir.join(format!("{role}{}", std::env::consts::EXE_SUFFIX));
        let mut put = PUT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !put.iter().any(|known| known == role) {
            // Beside the new one first, then renamed over: a program another test still runs is never written into.
            let fresh = dir.join(format!("{role}.{}.new", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(built(), &fresh).unwrap();
            if std::fs::rename(&fresh, &program).is_err() {
                let _ = std::fs::remove_file(&fresh);
                assert!(program.is_file(), "{} not put", program.display());
            }
            put.push(role.to_string());
        }
        program
    }

    /// The stub as the build made it.
    /// Looked for up from the test's own program, not at a fixed depth: cargo up to 1.99 puts a library's tests in
    /// `target/<profile>/deps/`, 1.100 in `target/<profile>/build/duowave/<hash>/out/` — the stub stays in
    /// `target/<profile>/` either way (0.18.0's CI, 22.11: 42 tests on Linux could not find it).
    pub fn built() -> PathBuf {
        let exe = std::env::current_exe().expect("the test's own path");
        let name = format!("duowave-test-stub{}", std::env::consts::EXE_SUFFIX);
        let found = exe.ancestors().skip(1).map(|dir| dir.join(&name)).find(|candidate| candidate.is_file());
        found.unwrap_or_else(|| {
            panic!(
                "no {name} above {}: `cargo test -p duowave` builds it; for `--lib` alone, first `cargo build -p \
                 duowave --bin duowave-test-stub`",
                exe.display()
            )
        })
    }
}
