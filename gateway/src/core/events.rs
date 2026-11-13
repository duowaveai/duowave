//! What happens in the gateway, as typed events: the audit log writes the admin's changes from them, the panel
//! follows them live, and scenarios will start from them. Payloads carry ids and names, never secrets, recognised
//! speech only where the event is about it.
//!
//! The bus is in-process publish/subscribe over a broadcast channel: delivery is after the publisher's call (the
//! listener runs in its own task), so a publisher that changes state first and publishes after is never re-entered
//! by its own listeners, and a listener's failure never reaches the publisher.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::identity::principal::{Principal, PrincipalKind};
use super::requests::RequestView;
use super::stations::presence::StationPresence;
use crate::skills::capabilities::CapabilityName;

/// Who caused an event: the principal's kind, id and name, as the audit log keeps them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: PrincipalKind,
    pub id: String,
    pub name: String,
}

impl From<&Principal> for Actor {
    fn from(principal: &Principal) -> Actor {
        Actor { kind: principal.kind, id: principal.id.clone(), name: principal.name.clone() }
    }
}

macro_rules! events {
    ($($(#[$vmeta:meta])* $variant:ident = $name:literal { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? })+) => {
        /// An event with its payload; serialized as `{name, payload}`, the payload's fields in camelCase.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "name", content = "payload")]
        pub enum GatewayEvent {
            $(
                $(#[$vmeta])*
                #[serde(rename = $name, rename_all = "camelCase")]
                $variant { $($(#[$meta])* $field: $ty),* },
            )+
        }

        impl GatewayEvent {
            /// The event's name: `settings.changed`, `station.connected`, …
            pub fn name(&self) -> &'static str {
                match self { $(GatewayEvent::$variant { .. } => $name,)+ }
            }
        }
    };
}

events! {
    /// A stored setting changed (global, or one person's). Values are not carried: some are sensitive.
    SettingsChanged = "settings.changed" {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] user_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    ModeChanged = "mode.changed" {
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
        /// The gateway switched by itself (src/placement/state.rs), and why: «компьютер не на связи».
        #[serde(default, skip_serializing_if = "Option::is_none")] auto: Option<String>,
    }
    RoutesChanged = "routes.changed" {
        routes: std::collections::BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// Whose voice speaks and which of ours.
    VoiceChanged = "voice.changed" {
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] model_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] voice_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    PersonaChanged = "persona.changed" {
        preset_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    ModelInstalled = "model.installed" {
        kind: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    ModelRemoved = "model.removed" {
        kind: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    ModelServerRestarted = "model.server.restarted" {
        which: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// An engine for the owner's own voices installed (src/voice_lab), and where it computes.
    VoiceEngineInstalled = "voice.engine.installed" {
        engine: String,
        device: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    VoiceOwnCreated = "voice.own.created" {
        id: String,
        engine: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    VoiceOwnRenamed = "voice.own.renamed" {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    VoiceOwnDeleted = "voice.own.deleted" {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// An install or an imprint ended: done, failed, or stopped (by the owner or the memory guard: `reason`).
    VoiceJobFinished = "voice.job.finished" {
        kind: String,
        engine: String,
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// The model providers or the choice of models changed: ids only, never keys.
    BrainChanged = "brain.changed" {
        change: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] provider_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// An MCP server was added, changed, removed, signed in, or the owner approved its tools or instructions.
    McpChanged = "mcp.changed" {
        change: String,
        server_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// A package was switched on or off, reloaded, its settings changed — or, without an actor, a plugin's code
    /// loaded, unloaded or failed (src/plugins). `error` says what failed, never a secret.
    PackagesChanged = "packages.changed" {
        change: String,
        package_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// A plugin wrote or removed its data (`host.kv`): the panel's widgets of the package read theirs again — a list
    /// filled by voice shows at once. At most once a second per package, the last write of a burst told too
    /// (src/plugins/host_api.rs). No keys, no values.
    PackageDataChanged = "packages.data" {
        package: String,
    }
    /// The MIDI keyboards plugged into this computer changed (src/midi): their names only — never a note.
    MidiDevices = "midi.devices" {
        devices: Vec<String>,
    }
    /// A draft of the workshop changed by its agent or by voice (src/plugins/workshop): the panel reads it again. The
    /// change: `agent` (a step of the agent), `voice.*`, and the draft's card (docs/plans/voice-creation.md) — `made`,
    /// `ready`, `failed`, `stopped`, `parked`, `installed`, `deleted`; `open:fitting` and `open:consent` when a station
    /// asks the panel to open «Примерка» or the «да».
    WorkshopChanged = "workshop.changed" {
        draft: String,
        change: String,
    }
    /// The plugins' memory changed enough to tell (src/plugins/budget.rs): what is free of their budget, the
    /// pressure. «Ресурсы колонки» reads the rest through the API.
    PluginsResources = "plugins.resources" {
        free_mb: u64,
        pressure: String,
    }
    /// The packages' catalog offers newer versions of installed packages (src/plugins/catalog.rs): each version told
    /// once. Nothing is installed by itself: a person with the right updates.
    PackagesCatalog = "packages.catalog" {
        updates: Vec<String>,
    }
    /// A connector set up in the panel changed (src/connectors/settings.rs): which fields, never their values.
    ConnectorChanged = "connector.changed" {
        connector: String,
        fields: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// The characters' memory changed (src/memory): what was done to which record, never its words — an erased
    /// fact must not stay behind in the audit log. A person's change carries them as the actor; what the household
    /// said or the model remembered carries none (the panel follows it live, the audit log leaves it out).
    MemoryChanged = "memory.changed" {
        action: String,
        target: String,
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// A night of the sleep (src/sleep) started, finished or was rolled back. A person who pressed «Запустить сон
    /// сейчас» or «Откатить ночь» is the actor; the night's own timer is not.
    SleepChanged = "sleep.changed" {
        run_id: String,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    UserCreated = "user.created" {
        user_id: String,
        name: String,
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    UserUpdated = "user.updated" {
        user_id: String,
        name: String,
        changes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    UserDeleted = "user.deleted" {
        user_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    ApiKeyCreated = "apiKey.created" {
        key_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    ApiKeyRevoked = "apiKey.revoked" {
        key_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    AuthLogin = "auth.login" {
        user_id: String,
        name: String,
        source: String,
    }
    AuthFailed = "auth.failed" {
        #[serde(default, skip_serializing_if = "Option::is_none")] name: Option<String>,
        source: String,
    }
    StationConnected = "station.connected" {
        session_id: String,
        device_tag: String,
    }
    /// A station the gateway does not know asked to connect: the admin approves it in the panel.
    StationPending = "station.pending" {
        device: String,
        address: String,
    }
    StationUpdated = "station.updated" {
        device: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] name: Option<String>,
        /// Absent: unchanged; null: back to the default.
        #[serde(default, skip_serializing_if = "Option::is_none")] model: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")] color: Option<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// The station said who it is: its model and colour at sync, its firmware in its spotter's log (logSpotter).
    /// What the station plays or which timers it holds changed (its state in a request, our player's news): the
    /// panel's widgets read it again (docs/plans/desk.md, stage 2).
    StationStateChanged = "station.state" {
        device: String,
        what: String,
    }
    StationSynced = "station.synced" {
        device_tag: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] device_model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] device_color: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] firmware: Option<String>,
    }
    /// A station came with another firmware than at its last sync: Yandex updated it (src/stations/firmware.rs).
    StationFirmwareChanged = "station.firmware" {
        device: String,
        from: String,
        to: String,
        /// Whether the gateway was tried with the new one; absent when nothing is known of the model's.
        #[serde(default, skip_serializing_if = "Option::is_none")] tried: Option<bool>,
    }
    StationDisconnected = "station.disconnected" {
        session_id: String,
        device_tag: String,
        /// How its socket closed: the close code of the handshake (1000 for a mode switch the station answered), or
        /// 1006 when it broke without one (a reset, the station gone silent and cut).
        #[serde(default, skip_serializing_if = "Option::is_none")] code: Option<u16>,
    }
    /// The panel switched a station's voice server over SSH: to the gateway, or back (src/stations/connect).
    StationSwitched = "station.switched" {
        address: String,
        to: String,
        result: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] device: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// One of the station's own clients came or went (station-client/): its microphone bridge, its call client.
    StationChannel = "station.channel" {
        address: String,
        channel: String,
        up: bool,
    }
    /// How a station is reachable now: its voice socket, its own clients, or nothing.
    StationPresenceChanged = "station.presence" {
        #[serde(flatten)] presence: StationPresence,
    }
    /// A request found its executor (or none): what scenarios and statistics hear.
    RequestRouted = "request.routed" {
        device_tag: String,
        capability: CapabilityName,
        executor: String,
        principal_id: String,
        source: String,
    }
    /// A request as it goes, whole each time: started, each step, the words heard and said, the end.
    RequestUpdated = "request.updated" {
        request: RequestView,
    }
    /// A new version is offered (src/updates): the panel shows it, the app's tray too.
    UpdateAvailable = "update.available" {
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] notes: Option<String>,
    }
    /// What works where changed (src/placement/availability.rs): the features whose state is new.
    AvailabilityChanged = "availability.changed" {
        features: Vec<String>,
    }
    /// The station's gateway sees its home server again (src/home_server).
    HomeServerUp = "homeServer.up" {
        url: String,
    }
    /// The station's gateway lost its home server: two checks in a row did not answer.
    HomeServerDown = "homeServer.down" {
        url: String,
        reason: String,
    }
    /// The home server's connection was set up, changed or removed in the panel.
    HomeServerChanged = "homeServer.changed" {
        configured: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// The gateway moved between the computer and the station, or gave the station back to Yandex (src/placement):
    /// started, done, failed.
    PlacementMoved = "placement.moved" {
        to: String,
        result: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] address: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// What the computer knows of the station's gateway changed: it came, went, updated, a settings copy was taken.
    PlacementChanged = "placement.changed" {
        what: String,
    }
    /// The gateway's own state changed: who chose the mode, the safety net, where the main settings live.
    GatewayStateChanged = "gateway.state" {
        what: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// The safety net on the station acted: the previous version back, or the station back to its previous address.
    SafetyFired = "safety.fired" {
        what: String,
        at: String,
    }
    /// The station's gateway was updated from the computer (src/updates): the version, and how it went.
    StationGatewayUpdated = "stationGateway.updated" {
        version: String,
        result: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// The bus's links, modes or the owner's words changed, or a question got its answer (src/bus): what and whose,
    /// never more. A person's change carries them as the actor.
    BusChanged = "bus.changed" {
        change: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<Actor>,
    }
    /// An app reached for something it may only have with a person's «да» (src/bus/consent.rs): the panel shows the
    /// question. Addresses only; the question itself is read through the API with the right to answer.
    BusAsked = "bus.asked" {
        id: String,
        from: String,
        to: String,
    }
    /// A participant told news on the bus: who and which — never what it carries. Its faces read theirs again.
    BusNews = "bus.news" {
        from: String,
        event: String,
    }
}

impl GatewayEvent {
    /// The payload alone, as the audit log and the panel's live channel carry it.
    pub fn payload(&self) -> serde_json::Value {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(mut object)) => object.remove("payload").unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        }
    }
}

/// The bus. Cheap to clone; every clone publishes to the same listeners.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Arc<GatewayEvent>>,
}

impl Default for EventBus {
    fn default() -> Self {
        EventBus::new()
    }
}

impl EventBus {
    pub fn new() -> EventBus {
        // Room for a burst (a request publishes on every step); a listener that falls further behind loses the
        // oldest and is told so.
        let (sender, _) = broadcast::channel(1024);
        EventBus { sender }
    }

    /// Publishes; nobody listening is fine.
    pub fn emit(&self, event: GatewayEvent) {
        let _ = self.sender.send(Arc::new(event));
    }

    /// Every event from now on, in order.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<GatewayEvent>> {
        self.sender.subscribe()
    }

    /// Runs `listener` for every event in its own task until the bus is gone. A listener that falls behind skips
    /// what it missed (logged), it never blocks the publishers.
    pub fn on<F>(&self, label: &'static str, mut listener: F) -> tokio::task::JoinHandle<()>
    where
        F: FnMut(Arc<GatewayEvent>) + Send + 'static,
    {
        let mut receiver = self.subscribe();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => listener(event),
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(target: "events", "{label} missed {missed} event(s)");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_name_and_camel_case_payload() {
        let event = GatewayEvent::SettingsChanged { key: "routes".into(), user_id: Some("u1".into()), actor: None };
        assert_eq!(event.name(), "settings.changed");
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({"name": "settings.changed", "payload": {"key": "routes", "userId": "u1"}})
        );
        assert_eq!(event.payload(), serde_json::json!({"key": "routes", "userId": "u1"}));
    }

    #[tokio::test]
    async fn delivers_after_the_publisher_in_order() {
        let bus = EventBus::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        bus.on("test", move |event| {
            let _ = tx.send(event.name());
        });
        bus.emit(GatewayEvent::AuthFailed { name: None, source: "panel".into() });
        bus.emit(GatewayEvent::StationPending { device: "d".into(), address: "a".into() });
        assert_eq!(rx.recv().await, Some("auth.failed"));
        assert_eq!(rx.recv().await, Some("station.pending"));
    }
}
