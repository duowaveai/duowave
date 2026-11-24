//! The station's build's own whole-gateway tests: what stands in for the computer's parts there.

mod models;
mod voice_lab;

/// The computer's own commands (src/computer/tools): the station's build says it has none.
#[test]
fn the_computers_commands_say_the_station_has_none() {
    let refused = crate::computer::tools::run("call-sim", &[]).expect("a command it knows").unwrap_err();
    assert!(refused.to_string().contains("no calls"), "{refused}");
    assert!(crate::computer::tools::run("dance", &[]).is_none(), "not a command of a computer's either");
}
