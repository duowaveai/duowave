//! «Джарвис, включи VPN» (clean-path check 18.11, item 30): the VPN window's big switch as the brain's tools, given the
//! way the screen and the memory give theirs (src/screen/tools.rs, src/memory/tools.rs).
//!
//! - `vpn_on`, `vpn_off` — what «Вкл» and «Выкл» in the VPN window do: `PUT /api/v1/egress {on}` → `Egress::set_on`,
//!   the same function, the speaker as the journal's actor (the setting's change names them). «Вкл» takes the pick and
//!   «напрямую …» as kept; «Выкл» — everything directly. Like the window's «Вкл», it needs a connection the app gave.
//! - `vpn_status` — on or off, and through which country and server.
//!
//! Offered only while the app that gives the connections (the MagicPlace app «VPN», `own-server`) is installed. Every
//! answer is a short true line the model retells: «включён» only when the switch is on now.
//!
//! The right: the use of apps by voice (`use:plugins`) — a household member may switch it, a guest may not; the
//! panel's switch itself asks for `settings.edit` (owner's decision 18.11: by voice — everyone at home).

use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Map, Value};

use super::connections::Connection;
use super::rules::Mode;
use super::{Egress, Table};
use crate::core::connectors::{
    Connector, ConnectorKind, ConnectorManifest, ConnectorStatus, ToolContext, ToolContribution, ToolRun,
};
use crate::core::events::Actor;
use crate::core::identity::permissions::use_of;
use crate::dialog::llm::LlmTool;
use crate::skills::capabilities::CapabilityName;

/// The installed app that gives the connections, if any.
type AppOf = Arc<dyn Fn(&Egress) -> Option<String> + Send + Sync>;

pub struct VpnConnector {
    egress: Weak<Egress>,
    app: AppOf,
}

impl VpnConnector {
    pub fn new(egress: &Arc<Egress>) -> VpnConnector {
        VpnConnector { egress: Arc::downgrade(egress), app: Arc::new(|egress: &Egress| egress.connections_app()) }
    }

    /// With the app's presence told (the tests have no plugins).
    #[cfg(test)]
    fn with_app(egress: &Arc<Egress>, app: AppOf) -> VpnConnector {
        VpnConnector { egress: Arc::downgrade(egress), app }
    }

    /// The service, while the VPN app is installed.
    fn installed(&self) -> Option<Arc<Egress>> {
        let egress = self.egress.upgrade()?;
        (self.app)(&egress).is_some().then_some(egress)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    On,
    Off,
    Status,
}

impl Action {
    fn tool(self) -> LlmTool {
        let (name, description) = match self {
            Action::On => (
                "vpn_on",
                "Turn the home's VPN on — the same as the switch «Вкл» in the VPN app's window: DuoWave goes to the \
                 internet through the person's own server, as the app's settings say. Call it for «включи VPN». The \
                 answer says whether it is on now and through which country; say only that.",
            ),
            Action::Off => (
                "vpn_off",
                "Turn the home's VPN off — the switch «Выкл» in the VPN app's window: everything goes to the internet \
                 directly. Call it for «выключи VPN». The answer says whether it is off now; say only that.",
            ),
            Action::Status => (
                "vpn_status",
                "Whether the home's VPN is on now, and through which country and server. Call it for «VPN включён?», \
                 «через какую страну VPN?».",
            ),
        };
        LlmTool {
            name: name.into(),
            description: description.into(),
            parameters: json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false }),
        }
    }
}

fn run(egress: &Weak<Egress>, app: &AppOf, action: Action) -> ToolRun {
    let (egress, app) = (egress.clone(), app.clone());
    Arc::new(move |_input: Map<String, Value>, context: ToolContext| -> BoxFuture<'static, anyhow::Result<String>> {
        let (egress, app) = (egress.clone(), app.clone());
        async move {
            let egress = egress.upgrade().ok_or_else(|| anyhow::anyhow!("the gateway is stopping"))?;
            if (app)(&egress).is_none() {
                return Ok(not_installed());
            }
            answer(&egress, action, Some(Actor::from(&context.principal)))
        }
        .boxed()
    })
}

/// Does what the switch does and says what is true after it.
fn answer(egress: &Egress, action: Action, actor: Option<Actor>) -> anyhow::Result<String> {
    let table = egress.table();
    let on = table.mode != Mode::Off;
    match action {
        Action::Status => Ok(status(&table)),
        Action::Off if !on => Ok(crate::tr!("VPN уже выключен: всё идёт напрямую.")),
        Action::Off => {
            egress.set_on(false, actor).map_err(|error| anyhow::anyhow!(error.message))?;
            if egress.table().mode != Mode::Off {
                anyhow::bail!("the switch did not turn off");
            }
            Ok(off_line())
        }
        // The window's «Вкл» waits for a connection too: on without one, nothing would go through a server.
        Action::On if table.connections.is_empty() => Ok(crate::tr!(
            "VPN не включён: приложение VPN ещё не дало шлюзу ни одного подключения — откройте окно VPN."
        )),
        Action::On if on => Ok(on_line(&table, true)),
        Action::On => {
            egress.set_on(true, actor).map_err(|error| anyhow::anyhow!(error.message))?;
            let table = egress.table();
            if table.mode == Mode::Off {
                anyhow::bail!("the switch did not turn on");
            }
            Ok(on_line(&table, false))
        }
    }
}

fn not_installed() -> String {
    crate::tr!("VPN не поставлен: приложение «VPN» ставят в MagicPlace.")
}

fn off_line() -> String {
    crate::tr!("VPN выключен: всё идёт напрямую.")
}

/// The state as the window's title says it: not set up, off, on through a place, on with its server silent.
fn status(table: &Table) -> String {
    if table.connections.is_empty() {
        return crate::tr!("VPN не настроен: приложение VPN ещё не дало шлюзу ни одного подключения.");
    }
    if table.mode == Mode::Off {
        return off_line();
    }
    on_line(table, false)
}

fn on_line(table: &Table, already: bool) -> String {
    let Some(main) = main_connection(table) else { return status(table) };
    let place = place(main);
    match (already, main.up()) {
        (false, true) => crate::tr!("VPN включён: {place}.", place = place),
        (true, true) => crate::tr!("VPN уже включён: {place}.", place = place),
        (false, false) => crate::tr!("VPN включён, но сервер сейчас не отвечает: {place}.", place = place),
        (true, false) => crate::tr!("VPN уже включён, но сервер сейчас не отвечает: {place}.", place = place),
    }
}

/// The connection the window shows (panel/src/screens/egress.tsx, `mainConnection`): the one chosen, else the first
/// that answers, else the first.
fn main_connection(table: &Table) -> Option<&Connection> {
    let list = &table.connections;
    table
        .preferred
        .as_deref()
        .and_then(|id| list.iter().find(|one| one.id == id))
        .or_else(|| list.iter().find(|one| one.up()))
        .or_else(|| list.first())
}

/// «Нидерланды — Амстердам»: the exit country and the current server without its flag; what is known of them.
fn place(connection: &Connection) -> String {
    let country = connection.country.as_deref().map(country_name);
    let server = connection
        .current
        .as_deref()
        .map(|name| name.trim_start_matches(|c: char| !c.is_alphanumeric()).trim())
        .filter(|name| !name.is_empty());
    match (country, server) {
        (Some(country), Some(server)) => format!("{country} — {server}"),
        (Some(country), None) => country,
        (None, Some(server)) => server.to_string(),
        (None, None) => format!("«{}»", connection.name),
    }
}

/// The exit countries a VPN is usually in, by ISO code: Russian, English.
const COUNTRIES: &[(&str, &str, &str)] = &[
    ("AE", "ОАЭ", "the UAE"),
    ("AM", "Армения", "Armenia"),
    ("AT", "Австрия", "Austria"),
    ("BG", "Болгария", "Bulgaria"),
    ("CA", "Канада", "Canada"),
    ("CH", "Швейцария", "Switzerland"),
    ("CY", "Кипр", "Cyprus"),
    ("CZ", "Чехия", "Czechia"),
    ("DE", "Германия", "Germany"),
    ("DK", "Дания", "Denmark"),
    ("EE", "Эстония", "Estonia"),
    ("ES", "Испания", "Spain"),
    ("FI", "Финляндия", "Finland"),
    ("FR", "Франция", "France"),
    ("GB", "Великобритания", "the UK"),
    ("GE", "Грузия", "Georgia"),
    ("HK", "Гонконг", "Hong Kong"),
    ("IT", "Италия", "Italy"),
    ("JP", "Япония", "Japan"),
    ("KZ", "Казахстан", "Kazakhstan"),
    ("LT", "Литва", "Lithuania"),
    ("LV", "Латвия", "Latvia"),
    ("MD", "Молдова", "Moldova"),
    ("NL", "Нидерланды", "the Netherlands"),
    ("NO", "Норвегия", "Norway"),
    ("PL", "Польша", "Poland"),
    ("PT", "Португалия", "Portugal"),
    ("RO", "Румыния", "Romania"),
    ("RS", "Сербия", "Serbia"),
    ("RU", "Россия", "Russia"),
    ("SE", "Швеция", "Sweden"),
    ("SG", "Сингапур", "Singapore"),
    ("TR", "Турция", "Turkey"),
    ("US", "США", "the US"),
];

/// A country's name in the language of now; one not in the list — its code.
fn country_name(code: &str) -> String {
    let english = crate::locale::text::language_now().is_some_and(|language| language != crate::locale::SOURCE);
    COUNTRIES
        .iter()
        .find(|(iso, _, _)| iso.eq_ignore_ascii_case(code))
        .map(|(_, russian, english_name)| if english { *english_name } else { *russian })
        .unwrap_or(code)
        .to_string()
}

impl Connector for VpnConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "vpn".into(),
            kind: ConnectorKind::Lookup,
            title: crate::tr!("VPN"),
            description: crate::tr!("Включить, выключить и проверить VPN голосом — как переключатель в окне VPN"),
            cloud: false,
            env: Vec::new(),
            // It serves no capability of the routes: its tools are the brain's own, not a station's skill.
            capabilities: None,
            setup: None,
        }
    }

    fn listed(&self) -> bool {
        false
    }

    fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
        Box::pin(async move {
            match self.installed() {
                Some(egress) => ConnectorStatus::ready(status(&egress.table())),
                None => ConnectorStatus::off(not_installed()),
            }
        })
    }

    fn tools(&self) -> Vec<ToolContribution> {
        if self.installed().is_none() {
            return Vec::new();
        }
        [(Action::On, false), (Action::Off, false), (Action::Status, true)]
            .into_iter()
            .map(|(action, read_only)| ToolContribution {
                app: None,
                tool: action.tool(),
                permission: use_of(CapabilityName::Plugins),
                run: run(&self.egress, &self.app, action),
                read_only,
                outside: false,
                spoken: false,
                kept: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::roles::Role;
    use crate::egress::tests::{egress, letter, Fresh, TURN};
    use crate::egress::Pick;

    fn with_vpn(egress: &Arc<Egress>) -> VpnConnector {
        VpnConnector::with_app(egress, Arc::new(|_: &Egress| Some("own-server".to_string())))
    }

    fn context() -> ToolContext {
        ToolContext {
            principal: household_principal(Role::Member, None),
            cancel: tokio_util::sync::CancellationToken::new(),
            station: None,
            origin: None,
        }
    }

    async fn call(connector: &VpnConnector, name: &str) -> String {
        let tools = connector.tools();
        let tool = tools.iter().find(|tool| tool.tool.name == name).unwrap_or_else(|| panic!("no tool {name}"));
        (tool.run)(Map::new(), context()).await.unwrap()
    }

    #[tokio::test]
    async fn without_the_vpn_app_the_tools_are_not_offered() {
        let _turn = TURN.lock().await;
        let _fresh = Fresh;
        let (egress, _settings, _dir) = egress(&[]);
        assert!(VpnConnector::new(&egress).tools().is_empty(), "no plugins: no app «VPN»");
        let none = VpnConnector::with_app(&egress, Arc::new(|_: &Egress| None));
        assert!(none.tools().is_empty());
        let names: Vec<String> = with_vpn(&egress).tools().into_iter().map(|tool| tool.tool.name).collect();
        assert_eq!(names, ["vpn_on", "vpn_off", "vpn_status"]);
        let status = with_vpn(&egress).tools().into_iter().find(|tool| tool.tool.name == "vpn_status").unwrap();
        assert!(status.read_only, "the status changes nothing");
    }

    #[tokio::test]
    async fn the_tools_turn_the_egress_on_and_off_as_the_switch_does() {
        let _turn = TURN.lock().await;
        let _fresh = Fresh;
        let (egress, _settings, _dir) = egress(&[]);
        let vpn = with_vpn(&egress);

        // No connection yet: «Вкл» does not turn on, as the window's button waits for one.
        egress.set_on(false, None).unwrap();
        let refused = call(&vpn, "vpn_on").await;
        assert!(refused.starts_with("VPN"), "{refused}");
        assert_eq!(egress.table().mode, Mode::Off, "nothing turned on without a connection");

        egress.report("own-server", &letter(9, true)).unwrap();
        egress.rebuild();
        // The pick kept while off comes back with «Вкл», as with the switch.
        egress.set_pick(Pick::Only, None).unwrap();
        assert_eq!(egress.table().mode, Mode::Off);
        call(&vpn, "vpn_on").await;
        assert_eq!(egress.table().mode, Mode::Only, "on: the mode of the pick kept");
        assert_eq!(super::super::routes::view_of(&egress)["on"], true);

        let again = call(&vpn, "vpn_on").await;
        assert_eq!(egress.table().mode, Mode::Only, "already on: nothing changes ({again})");

        call(&vpn, "vpn_off").await;
        assert_eq!(egress.table().mode, Mode::Off);
        assert_eq!(super::super::routes::view_of(&egress)["on"], false);
        let status = call(&vpn, "vpn_status").await;
        assert_eq!(status, off_line());

        // The same mode the panel's switch sets.
        egress.set_pick(Pick::All, None).unwrap();
        call(&vpn, "vpn_on").await;
        let by_voice = egress.table().mode;
        egress.set_on(false, None).unwrap();
        egress.set_on(true, None).unwrap();
        assert_eq!(egress.table().mode, by_voice);
    }

    #[tokio::test]
    async fn the_answers_say_what_is_true() {
        let _turn = TURN.lock().await;
        let _fresh = Fresh;
        let (egress, _settings, _dir) = egress(&[]);
        egress.report("own-server", &letter(9, true)).unwrap();
        egress.set_on(true, None).unwrap();
        let table = egress.table();
        let main = main_connection(&table).unwrap();
        assert!(main.up());
        assert_eq!(place(main), format!("{} — Амстердам", country_name("NL")), "the flag is not said");
        let up = status(&table);
        assert_ne!(up, off_line());

        egress.report("own-server", &letter(9, false)).unwrap();
        egress.rebuild();
        let table = egress.table();
        assert!(!main_connection(&table).unwrap().up());
        assert_ne!(status(&table), up, "a silent server is not said as working");
        assert_eq!(country_name("XX"), "XX");
    }
}
