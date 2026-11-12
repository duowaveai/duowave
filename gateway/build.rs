//! Build steps: the station's protocol from proto/station-protocol.proto (protox, no protoc needed) as prost types
//! and as an encoded descriptor set for the codec's reflection, the list of SQL migrations in migrations/ (adding a
//! migration is adding a file), an empty panel folder when the panel is not built, so the program still builds
//! (it then says the panel is missing), and the station's build inside the computer's when one is given
//! (DUOWAVE_STATION_PAYLOAD: duowave-station-aarch64.tar.gz with its .sig beside it; src/placement/payload.rs); and,
//! for recognition on a Mac (feature local-asr), clang's runtime that ggml-metal's version checks call.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?);

    println!("cargo:rerun-if-changed=proto");
    let mut compiler = protox::Compiler::new([manifest.join("proto")])?;
    compiler.include_imports(true).include_source_info(true).open_files(["station-protocol.proto"])?;
    let mut prost = prost_build::Config::new();
    prost.bytes(["."]);
    prost.compile_fds(compiler.file_descriptor_set())?;
    // The schema the codec reads plain objects by (gateway/protocol/proto_object.rs), without the comments.
    compiler.include_source_info(false);
    std::fs::write(out.join("station-protocol.fds"), compiler.encode_file_descriptor_set())?;

    println!("cargo:rerun-if-changed=migrations");
    std::fs::write(out.join("migrations.rs"), migrations(&manifest.join("migrations"))?)?;

    let panel = manifest.join("../panel/dist");
    if !panel.exists() {
        std::fs::create_dir_all(&panel)?;
    }
    println!("cargo:rerun-if-changed=../panel/dist");

    std::fs::write(out.join("station_payload.rs"), station_payload(&out)?)?;
    link_clang_rt_osx();
    Ok(())
}

/// ggml-metal (transcribe.cpp, feature local-asr) guards newer Metal calls with `@available`. Built for a macOS older
/// than the SDK — the app's minimum is 13.5 — clang turns those checks into calls to `__isPlatformVersionAtLeast` in
/// its runtime, libclang_rt.osx.a, which rustc does not link (`-nodefaultlibs`): the app's link failed without it
/// (12.11). Linked here, as Handy does; a build for the host's own macOS folds the checks away and needs nothing.
fn link_clang_rt_osx() {
    let mac = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if !mac || std::env::var_os("CARGO_FEATURE_LOCAL_ASR").is_none() {
        return;
    }
    let resource_dir = std::process::Command::new("clang")
        .arg("--print-resource-dir")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let Some(resource_dir) = resource_dir else {
        println!("cargo:warning=clang --print-resource-dir failed: libclang_rt.osx.a not linked");
        return;
    };
    let darwin = Path::new(&resource_dir).join("lib").join("darwin");
    if !darwin.join("libclang_rt.osx.a").exists() {
        println!("cargo:warning=no libclang_rt.osx.a in {}", darwin.display());
        return;
    }
    println!("cargo:rustc-link-search=native={}", darwin.display());
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}

/// The station's build carried by the computer's: the archive and its signature, both checked by the program before
/// anything goes to a station. The station's own build carries none, and neither does a build without the variable.
fn station_payload(out: &Path) -> Result<String, Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=DUOWAVE_STATION_PAYLOAD");
    let none = "/// No station build inside this one.\npub const BUILT_IN: Option<(&[u8], &str)> = None;\n".to_string();
    let Some(path) = std::env::var("DUOWAVE_STATION_PAYLOAD").ok().filter(|path| !path.is_empty()) else {
        return Ok(none);
    };
    if std::env::var_os("CARGO_FEATURE_STATION").is_some() {
        return Ok(none);
    }
    let signature = PathBuf::from(format!("{path}.sig"));
    println!("cargo:rerun-if-changed={path}");
    println!("cargo:rerun-if-changed={}", signature.display());
    let signature = std::fs::read_to_string(&signature)
        .map_err(|error| format!("DUOWAVE_STATION_PAYLOAD: {}: {error}", signature.display()))?;
    std::fs::copy(&path, out.join("station-payload.tar.gz"))
        .map_err(|error| format!("DUOWAVE_STATION_PAYLOAD: {path}: {error}"))?;
    Ok(format!(
        "/// The station's build inside this one: the archive and its signature.\n\
         pub const BUILT_IN: Option<(&[u8], &str)> = Some((include_bytes!(concat!(env!(\"OUT_DIR\"), \"/station-payload.tar.gz\")), {:?}));\n",
        signature.trim()
    ))
}

/// `NNNN_name.sql`, in order, each once: the program refuses a gap or a repeated number at build time.
fn migrations(dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut found: Vec<(u32, String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(file) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let Some(stem) = file.strip_suffix(".sql") else { continue };
        let (number, name) = stem.split_once('_').ok_or_else(|| format!("migration {file}: not NNNN_name.sql"))?;
        let version: u32 = number.parse().map_err(|_| format!("migration {file}: not NNNN_name.sql"))?;
        found.push((version, name.to_string(), path.clone()));
    }
    found.sort();
    for (index, (version, _, path)) in found.iter().enumerate() {
        if *version != index as u32 + 1 {
            return Err(format!("migration {}: expected number {}", path.display(), index + 1).into());
        }
    }
    let mut code =
        String::from("/// The SQL migrations, from migrations/, in order.\npub const MIGRATIONS: &[Migration] = &[\n");
    for (version, name, path) in found {
        writeln!(
            code,
            "    Migration {{ version: {version}, name: {name:?}, sql: include_str!({:?}) }},",
            path.display()
        )?;
    }
    code.push_str("];\n");
    Ok(code)
}
