//! The SQL migrations of gateway/migrations (adding a migration is adding a file), listed for the program in order: the
//! database is this crate's (src/storage).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    // The migrations stay where they always were, beside the gateway: the checks and the people who look for them there.
    let dir = manifest.join("../../gateway/migrations");
    println!("cargo:rerun-if-changed={}", dir.display());
    std::fs::write(out.join("migrations.rs"), migrations(&dir)?)?;
    Ok(())
}

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
