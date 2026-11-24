//! The Station's protocol from proto/station-protocol.proto (protox, no protoc needed): prost types, and an encoded
//! descriptor set for the codec's reflection (gateway/protocol/proto_object.rs).

use std::path::PathBuf;

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
    Ok(())
}
