//! Whether a station is there, as the gateway can tell without asking it anything: its voice socket (/uni.ws), and,
//! where the station runs our client (station-client/), its microphone bridge and its call client's poll. Those two
//! come back within seconds of a gateway restart, while the station opens its voice socket on its own schedule —
//! from ten seconds to minutes — or at once when someone says the wake word. Pure data.

use serde::{Deserialize, Serialize};

/// A channel of the station's own to the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StationChannel {
    Voice,
    Microphone,
    Calls,
}

/// voice — its voice socket is up; alive — its own clients are up, the voice socket opens with the next wake word or
/// on its own; reconnecting — the station is between sockets and opens the next by itself: its voice socket just
/// closed by a close handshake (a mode switch, its own close), or the gateway just started and the station was here
/// before it; offline — nothing heard, or its socket broke or went silent (it may be gone: a reboot, no network).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceState {
    Voice,
    Alive,
    Reconnecting,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationPresence {
    pub device: String,
    pub state: PresenceState,
    pub channels: Vec<StationChannel>,
    /// When anything of it was last heard (kept in its record, so a restart remembers it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    /// Since when it is in this state.
    pub since: String,
}

/// How long a call client's poll counts: it asks again every 50 s (src/calls/http_stations.rs), gone after 120 s.
pub const POLL_COUNTS_MS: u64 = 120_000;
/// After a restart, a station heard this recently before it is «reconnecting» for as long; and as long after its
/// voice socket closed by a handshake. It takes its time to open the next: after a mode switch from 1 s to 2.5 min
/// (median a minute, 23 times on 06–08.11.2026), at once with the wake word.
pub const RECONNECT_WINDOW_MS: u64 = 180_000;

/// A socket that ended with this code was closed by a handshake, from either side: the station opens a new one by
/// itself. 1006 is a socket that broke or was cut without one (a reset, the station silent to our pings).
pub fn closed_by_handshake(code: u16) -> bool {
    code != 1006
}

#[derive(Debug, Clone, Default)]
pub struct PresenceInput {
    pub voice: bool,
    pub microphone: bool,
    /// When its call client last polled (ms), if it does.
    pub polled_at: Option<u64>,
    /// When it was last heard before this gateway started (ms).
    pub seen_before_start: Option<u64>,
    /// When its last voice socket closed by a handshake (ms), none open since.
    pub voice_closed_at: Option<u64>,
    pub started_at: u64,
    pub now: u64,
}

/// The state and the channels up, from what the gateway hears of the station.
pub fn presence_of(input: &PresenceInput) -> (PresenceState, Vec<StationChannel>) {
    let mut channels = Vec::new();
    if input.voice {
        channels.push(StationChannel::Voice);
    }
    if input.microphone {
        channels.push(StationChannel::Microphone);
    }
    if input.polled_at.is_some_and(|polled| input.now.saturating_sub(polled) < POLL_COUNTS_MS) {
        channels.push(StationChannel::Calls);
    }
    if input.voice {
        return (PresenceState::Voice, channels);
    }
    if !channels.is_empty() {
        return (PresenceState::Alive, channels);
    }
    let just_started = input.now.saturating_sub(input.started_at) < RECONNECT_WINDOW_MS;
    let was_here =
        input.seen_before_start.is_some_and(|seen| input.started_at.saturating_sub(seen) < RECONNECT_WINDOW_MS);
    let just_closed =
        input.voice_closed_at.is_some_and(|closed| input.now.saturating_sub(closed) < RECONNECT_WINDOW_MS);
    (
        if (just_started && was_here) || just_closed { PresenceState::Reconnecting } else { PresenceState::Offline },
        channels,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTED: u64 = 1_000_000;

    fn base() -> PresenceInput {
        PresenceInput { started_at: STARTED, now: STARTED, ..Default::default() }
    }

    #[test]
    fn is_voice_while_the_voice_socket_is_up() {
        let input = PresenceInput { voice: true, microphone: true, now: STARTED + 5000, ..base() };
        assert_eq!(
            presence_of(&input),
            (PresenceState::Voice, vec![StationChannel::Voice, StationChannel::Microphone])
        );
    }

    #[test]
    fn is_alive_on_its_own_clients_alone() {
        let mic = PresenceInput { microphone: true, now: STARTED + 2000, ..base() };
        assert_eq!(presence_of(&mic), (PresenceState::Alive, vec![StationChannel::Microphone]));
        let polled = PresenceInput { polled_at: Some(STARTED + 1000), now: STARTED + 2000, ..base() };
        assert_eq!(presence_of(&polled), (PresenceState::Alive, vec![StationChannel::Calls]));
    }

    #[test]
    fn a_poll_counts_for_two_minutes() {
        let now = STARTED + 10 * 60_000;
        assert_eq!(
            presence_of(&PresenceInput { polled_at: Some(now - POLL_COUNTS_MS + 1), now, ..base() }).0,
            PresenceState::Alive
        );
        assert_eq!(
            presence_of(&PresenceInput { polled_at: Some(now - POLL_COUNTS_MS), now, ..base() }).0,
            PresenceState::Offline
        );
    }

    #[test]
    fn is_reconnecting_right_after_a_restart_for_a_station_that_was_here() {
        let now = STARTED + 1000;
        assert_eq!(
            presence_of(&PresenceInput { seen_before_start: Some(STARTED - 5000), now, ..base() }).0,
            PresenceState::Reconnecting
        );
        assert_eq!(
            presence_of(&PresenceInput { seen_before_start: Some(STARTED - RECONNECT_WINDOW_MS - 1), now, ..base() }).0,
            PresenceState::Offline
        );
        assert_eq!(presence_of(&PresenceInput { now, ..base() }).0, PresenceState::Offline);
        assert_eq!(
            presence_of(&PresenceInput {
                seen_before_start: Some(STARTED - 5000),
                now: STARTED + RECONNECT_WINDOW_MS,
                ..base()
            })
            .0,
            PresenceState::Offline
        );
    }

    #[test]
    fn is_reconnecting_for_a_while_after_its_voice_socket_closed_by_a_handshake() {
        // An hour into the gateway's run: the restart's window is long over.
        let closed = STARTED + 3_600_000;
        let after = |ms: u64| PresenceInput { voice_closed_at: Some(closed), now: closed + ms, ..base() };
        assert_eq!(presence_of(&after(0)), (PresenceState::Reconnecting, vec![]));
        assert_eq!(presence_of(&after(RECONNECT_WINDOW_MS - 1)).0, PresenceState::Reconnecting);
        assert_eq!(presence_of(&after(RECONNECT_WINDOW_MS)).0, PresenceState::Offline);
        // Its own clients still up: it is there, the window does not matter.
        let polled = PresenceInput { polled_at: Some(closed), ..after(1000) };
        assert_eq!(presence_of(&polled).0, PresenceState::Alive);
    }

    #[test]
    fn a_socket_that_broke_or_was_cut_was_not_closed_by_a_handshake() {
        assert!(closed_by_handshake(1000), "a mode switch the station answered");
        assert!(closed_by_handshake(1005), "a close without a code");
        assert!(closed_by_handshake(1011), "the relay's close when Yandex left");
        assert!(!closed_by_handshake(1006), "a reset, a silent station cut");
    }
}
