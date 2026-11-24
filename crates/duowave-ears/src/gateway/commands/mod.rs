//! The Station's adapter to the ear contract (src/ear, docs/plans/modular-gateway.md, «Договор уха»): each command the
//! brain gives an ear, as Yandex sends it to its Station — client actions (basic.rs), typed commands for timers, alarms
//! and «не беспокоить» (typed.rs), drawings on the display (display.rs) and the player's audio_play (player.rs).

pub mod basic;
pub mod display;
pub mod player;
pub mod typed;

use bytes::Bytes;

use crate::ear::Command;
use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};

/// The Station's commands for one of the contract's.
pub fn directives(command: &Command) -> Vec<Directive> {
    match command {
        Command::SetVolume { level, whisper } => vec![basic::set_volume(*level, *whisper)],
        Command::Louder => vec![basic::louder()],
        Command::Quieter => vec![basic::quieter()],
        Command::Mute => vec![basic::mute()],
        Command::Unmute => vec![basic::unmute()],
        Command::StopPlayer => basic::stop_player(),
        Command::PausePlayer => vec![basic::pause_player()],
        Command::ContinuePlayer => vec![basic::continue_player()],
        Command::NextTrack => vec![basic::next_track()],
        Command::PreviousTrack => vec![basic::previous_track()],
        Command::Play { item, play_id, offset_ms } => player::play(item, play_id, *offset_ms),
        Command::AskAgain { play_id } => vec![player::ask_again(play_id)],
        Command::StartBluetooth => vec![basic::start_bluetooth()],
        Command::StopBluetooth => vec![basic::stop_bluetooth()],
        Command::EndDialog { dialog_id } => basic::end_dialog(dialog_id),
        Command::TimerSet(timer) => vec![typed::timer_set(timer)],
        Command::TimerStop { id } => vec![typed::timer_stop(id)],
        Command::TimerCancel { id } => vec![typed::timer_cancel(id)],
        Command::TimerPause { id } => vec![typed::timer_pause(id)],
        Command::TimerResume { id } => vec![typed::timer_resume(id)],
        Command::AlarmAdd(alarm) => vec![typed::alarm_add(alarm)],
        Command::AlarmCancel { id } => vec![typed::alarm_cancel(id)],
        Command::AlarmStop => vec![typed::alarm_stop()],
        Command::AlarmSnooze(alarm) => typed::alarm_snooze(alarm).to_vec(),
        Command::DoNotDisturb(on) => vec![typed::do_not_disturb(*on)],
        Command::ShowTemperature(celsius) => display::show_temperature(*celsius).into_iter().collect(),
        Command::LightAnimation(payload) => vec![light_animation(payload)],
    }
}

/// The Station's commands for an answer's, in order.
pub fn directives_of(commands: &[Command]) -> Vec<Directive> {
    commands.iter().flat_map(directives).collect()
}

/// An animation of the lights as it was captured from the Station: its own `draw_animation_directive`, the payload as
/// it came (the LED skill's `.b64` files).
fn light_animation(payload: &[u8]) -> Directive {
    Directive {
        r#type: DirectiveType::ClientAction,
        name: "draw_animation_directive".into(),
        analytics_type: Some("draw_animation_directive".into()),
        payload: None,
        typed_payload: Some(Bytes::from(payload.to_vec())),
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}
