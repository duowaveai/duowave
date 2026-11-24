//! Build steps (the Station's protocol and the station's build carried inside are the crates' own:
//! crates/duowave-ears/build.rs, crates/duowave-home/build.rs): the panel's files packed for the program, its text
//! gzip-compressed (an empty panel folder when the panel is not built, so the program still builds and then says the
//! panel is missing); and, for recognition on a Mac (feature local-asr), clang's runtime that ggml-metal's version
//! checks call.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?);

    let panel = manifest.join("../panel/dist");
    if !panel.exists() {
        std::fs::create_dir_all(&panel)?;
    }
    println!("cargo:rerun-if-changed=../panel/dist");
    std::fs::write(out.join("panel_files.rs"), panel_files(&panel, &out)?)?;

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

/// The panel's files for the program (src/panel/controller.rs): all of panel/dist in one blob, with a table of where
/// each file lies. A file that gzip makes smaller by an eighth or more — scripts, styles, pages, SVG — is kept
/// compressed at the best level, as a browser takes it; fonts in woff2 and PNG pictures are compressed already and stay
/// as they are. The station keeps the program on a small flash (docs/plans/home-and-helpers.md: +300 KB a block at
/// most), and the panel's 1.5 MB of text goes in as about 0.45 MB. An empty panel/dist packs to nothing.
fn panel_files(dist: &Path, out: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    panel_folder(dist, dist, &mut files)?;
    files.sort();
    let mut blob = Vec::new();
    let mut code = String::from(
        "/// The panel's files packed from panel/dist, by path: where each lies in `BLOB` and whether it is gzip.\n\
         pub static FILES: &[Packed] = &[\n",
    );
    for (path, bytes) in files {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&bytes)?;
        let compressed = encoder.finish()?;
        let gzip = compressed.len() <= bytes.len() - bytes.len() / 8;
        let start = blob.len();
        blob.extend_from_slice(if gzip { &compressed } else { &bytes });
        writeln!(code, "    Packed {{ path: {path:?}, gzip: {gzip}, start: {start}, end: {} }},", blob.len())?;
    }
    code.push_str(
        "];\n\n/// Every packed file's bytes, one after another.\n\
         pub static BLOB: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/panel.bin\"));\n",
    );
    std::fs::write(out.join("panel.bin"), blob)?;
    Ok(code)
}

/// Every file under `folder` with its path from `root`, `/` between the parts; links are followed, as rust-embed does.
fn panel_folder(
    root: &Path,
    folder: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        if path.is_dir() {
            panel_folder(root, &path, files)?;
        } else if path.is_file() {
            let parts: Option<Vec<&str>> = path.strip_prefix(root)?.iter().map(|part| part.to_str()).collect();
            let relative = parts.ok_or_else(|| format!("panel/dist: {} is not UTF-8", path.display()))?.join("/");
            files.push((relative, std::fs::read(&path)?));
        }
    }
    Ok(())
}
