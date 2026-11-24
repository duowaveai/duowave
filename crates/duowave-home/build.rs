//! The station's build inside the computer's when one is given (DUOWAVE_STATION_PAYLOAD:
//! duowave-station-aarch64.tar.gz with its .sig beside it; placement/payload.rs).

use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out.join("station_payload.rs"), station_payload(&out)?)?;
    Ok(())
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
