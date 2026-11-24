//! Calls through a LiveKit room, without the SDK: who is in the room is who our hub can reach, and data messages
//! on topic `duowave` carry the calls — `call` (ring the station), `accept`, `hangup` from a participant;
//! `incoming`, `ringing`, `connected`, `ended` to it. The station's voice goes to the room while a call is on, a
//! participant's voice comes from its audio track. One family room: a call is one-to-one by agreement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

use crate::calls::call_hub::{
    js_string, Call, CallEndpoint, CallError, CallEvent, CallEventType, CallHub, EndpointKind,
};

/// The topic of our call signalling among the room's data messages.
pub const LIVEKIT_TOPIC: &str = "duowave";

/// What the bridge does with the room: a data message to one participant, the station's audio to the room.
pub trait RoomLink: Send + Sync {
    fn send(&self, identity: &str, message: Value);
    fn play(&self, pcm: &[i16]);
}

/// A participant of the LiveKit room in our hub: a phone with the app, a browser on the LiveKit page.
struct LiveKitEndpoint {
    id: String,
    name: String,
    identity: String,
    link: Arc<dyn RoomLink>,
    /// This connection of the identity: a newer one replaces it.
    sid: Option<String>,
}

impl CallEndpoint for LiveKitEndpoint {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> EndpointKind {
        EndpointKind::Livekit
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// The other party's audio: the station's voice, published to the room.
    fn deliver(&self, frame: &[i16]) {
        self.link.play(frame);
    }

    fn notify(&self, event: &CallEvent) {
        let kind = match event.kind {
            CallEventType::Incoming => "incoming",
            CallEventType::Ringing => "ringing",
            CallEventType::Connected => "connected",
            CallEventType::Ended => "ended",
        };
        let mut message = json!({ "type": kind, "callId": event.call_id, "peer": event.peer.name });
        if let Some(reason) = event.reason.as_deref().filter(|reason| !reason.is_empty()) {
            message["reason"] = json!(reason);
        }
        self.link.send(&self.identity, message);
    }
}

pub struct LiveKitCalls {
    hub: Arc<CallHub>,
    link: Arc<dyn RoomLink>,
    endpoints: Mutex<HashMap<String, Arc<LiveKitEndpoint>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl LiveKitCalls {
    pub fn new(hub: Arc<CallHub>, link: Arc<dyn RoomLink>) -> LiveKitCalls {
        LiveKitCalls { hub, link, endpoints: Mutex::default() }
    }

    pub fn joined(&self, identity: &str, name: Option<&str>, sid: Option<&str>) {
        let name = name.filter(|name| !name.is_empty()).unwrap_or(identity);
        let endpoint = Arc::new(LiveKitEndpoint {
            id: format!("livekit:{identity}"),
            name: name.to_string(),
            identity: identity.to_string(),
            link: self.link.clone(),
            sid: sid.map(str::to_string),
        });
        lock(&self.endpoints).insert(identity.to_string(), endpoint.clone());
        self.hub.register(endpoint);
    }

    /// A connection left; a newer one of the same identity (a second page) stays a party.
    pub fn left(&self, identity: &str, sid: Option<&str>) {
        {
            let mut endpoints = lock(&self.endpoints);
            let newer = endpoints.get(identity).and_then(|endpoint| endpoint.sid.as_deref());
            if let (Some(sid), Some(newer)) = (sid, newer) {
                if sid != newer {
                    return;
                }
            }
            endpoints.remove(identity);
        }
        self.hub.unregister(&format!("livekit:{identity}"), "left the room");
    }

    /// Every participant leaves the hub: the bridge leaves the room for good, its calls end with it.
    pub fn leave_all(&self, reason: &str) {
        let identities: Vec<String> = lock(&self.endpoints).drain().map(|(identity, _)| identity).collect();
        for identity in identities {
            self.hub.unregister(&format!("livekit:{identity}"), reason);
        }
    }

    /// A participant's voice: to the other side of its call, if one is on.
    pub fn audio(&self, identity: &str, pcm: &[i16]) {
        if lock(&self.endpoints).contains_key(identity) {
            self.hub.audio(&format!("livekit:{identity}"), pcm);
        }
    }

    pub fn message(&self, identity: &str, message: &Value) {
        let Some(endpoint) = lock(&self.endpoints).get(identity).cloned() else { return };
        if let Err(error) = self.control(&endpoint, message) {
            self.link.send(identity, json!({ "type": "error", "message": error.0 }));
        }
    }

    fn control(&self, endpoint: &LiveKitEndpoint, message: &Value) -> Result<(), CallError> {
        match message.get("type").and_then(Value::as_str) {
            Some("call") => {
                let target = match message.get("to").and_then(Value::as_str).filter(|to| !to.is_empty()) {
                    Some(to) => self.hub.find(to, Some(&endpoint.id)),
                    None => self.station(),
                };
                let target = target.ok_or_else(|| CallError("the station is not on our calls".into()))?;
                self.hub.call(&endpoint.id, target.id())?;
                Ok(())
            }
            Some("accept") => {
                let call_id = match message.get("callId") {
                    None | Some(Value::Null) => {
                        js_string(self.call_of(endpoint).map(|call| Value::String(call.id)).as_ref())
                    }
                    given => js_string(given),
                };
                self.hub.accept(&call_id, &endpoint.id)?;
                Ok(())
            }
            Some("hangup") => {
                if let Some(call) = self.call_of(endpoint) {
                    self.hub.hangup(&call.id, &endpoint.id)?;
                }
                Ok(())
            }
            _ => Err(CallError(format!("unknown message {}", js_string(message.get("type"))))),
        }
    }

    fn call_of(&self, endpoint: &LiveKitEndpoint) -> Option<Call> {
        self.hub.call_of(&endpoint.id)
    }

    fn station(&self) -> Option<Arc<dyn CallEndpoint>> {
        let station = self.hub.list().into_iter().find(|party| party.kind == EndpointKind::Station)?;
        self.hub.endpoint(&station.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calls::call_hub::tests::TestParty;

    #[derive(Default)]
    struct Link {
        sent: Mutex<Vec<(String, Value)>>,
        played: Mutex<Vec<Vec<i16>>>,
    }

    impl RoomLink for Link {
        fn send(&self, identity: &str, message: Value) {
            lock(&self.sent).push((identity.to_string(), message));
        }
        fn play(&self, pcm: &[i16]) {
            lock(&self.played).push(pcm.to_vec());
        }
    }

    impl Link {
        fn last(&self) -> (String, Value) {
            lock(&self.sent).last().cloned().unwrap()
        }
    }

    #[tokio::test]
    async fn makes_room_participants_parties_of_the_hub_and_carries_their_calls_by_data_messages() {
        let hub = CallHub::new();
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(station.clone());
        let link = Arc::new(Link::default());
        let calls = LiveKitCalls::new(hub.clone(), link.clone());

        calls.joined("app-mom", Some("Мама"), None);
        assert_eq!(hub.find("маме", None).map(|found| found.id().to_string()).as_deref(), Some("livekit:app-mom"));

        calls.message("app-mom", &json!({"type": "call"}));
        assert_eq!(station.last().kind, CallEventType::Incoming);
        assert_eq!(station.last().peer.name, "Мама");
        let (identity, message) = link.last();
        assert_eq!(identity, "app-mom");
        assert_eq!((message["type"].as_str(), message["peer"].as_str()), (Some("ringing"), Some("Колонка")));

        let call = hub.call_of(&station.id).unwrap();
        hub.accept(&call.id, &station.id).unwrap();
        assert_eq!(link.last().1["type"], "connected");

        // Mom's voice reaches the station, the station's reaches the room.
        calls.audio("app-mom", &[5; 320]);
        assert_eq!(station.frames().len(), 1);
        hub.audio(&station.id, &[9; 320]);
        assert_eq!(lock(&link.played).len(), 1);

        calls.message("app-mom", &json!({"type": "hangup"}));
        assert_eq!(station.last().kind, CallEventType::Ended);
        calls.message("app-mom", &json!({"type": "dance"}));
        assert_eq!(link.last().1, json!({"type": "error", "message": "unknown message dance"}));
        calls.left("app-mom", None);
        assert!(hub.find("маме", None).is_none());
    }

    #[tokio::test]
    async fn keeps_the_newer_one_a_party_when_the_older_leaves_after_it_joined() {
        let hub = CallHub::new();
        let calls = LiveKitCalls::new(hub.clone(), Arc::new(Link::default()));
        calls.joined("app-mom", Some("Мама"), Some("PA_old"));
        calls.joined("app-mom", Some("Мама"), Some("PA_new"));
        calls.left("app-mom", Some("PA_old"));
        assert_eq!(hub.find("маме", None).map(|found| found.id().to_string()).as_deref(), Some("livekit:app-mom"));
        calls.left("app-mom", Some("PA_new"));
        assert!(hub.find("маме", None).is_none());
    }

    #[tokio::test]
    async fn answers_a_call_without_its_id_and_tells_a_participant_what_went_wrong() {
        let hub = CallHub::new();
        let link = Arc::new(Link::default());
        let calls = LiveKitCalls::new(hub.clone(), link.clone());
        calls.joined("app-mom", None, None);
        assert_eq!(hub.list()[0].name, "app-mom", "no name: the identity");
        calls.message("app-mom", &json!({"type": "call"}));
        assert_eq!(link.last().1["message"], "the station is not on our calls");
        calls.message("app-mom", &json!({"type": "accept"}));
        assert_eq!(link.last().1["message"], "no such call ringing");

        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(station.clone());
        hub.call(&station.id, "livekit:app-mom").unwrap();
        calls.message("app-mom", &json!({"type": "accept"}));
        assert_eq!(station.last().kind, CallEventType::Connected);
        calls.message("nobody", &json!({"type": "hangup"}));
        assert_eq!(station.last().kind, CallEventType::Connected, "a stranger to the hub is not heard");
    }
}
