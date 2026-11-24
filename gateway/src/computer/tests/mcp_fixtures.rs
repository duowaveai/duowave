//! The MCP fixture programs of the program's own tests: this test binary run again as a server over stdio, by its
//! own test's name (mcp/fixtures.rs serves; the apps' crate has fixture programs of its own, under their names there).

/// The test that is the legacy fixture program here: `fixtures::program(LEGACY_PROGRAM)` runs this binary with it.
pub const LEGACY_PROGRAM: &str = "computer::tests::mcp_fixtures::stdio_legacy";

#[test]
#[ignore = "a fixture: the MCP tests run it as a program"]
fn stdio_legacy() {
    crate::mcp::fixtures::legacy_as(LEGACY_PROGRAM);
}
