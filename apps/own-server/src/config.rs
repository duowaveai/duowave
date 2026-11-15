//! The engine's settings (shoes YAML) for every connection: each server its own group (`<cid>-s<n>`), the fastest of
//! them (`<cid>-auto`, url-test), the person's choice over them (`<cid>-choice`, a selector, `auto` by default), and an
//! entrance `mixed` (HTTP + SOCKS) on `127.0.0.1:<port>` whose every request goes the choice's way — never directly:
//! a dead server is a failed request, not a request from the home's address. The Clash API, on its own local port with
//! a random secret, is for this program only.

use serde_json::{Value, json};

use crate::links::Server;

/// The probe of the fastest server: a name, not an address (shoes refuses literal addresses there, 15.11).
pub const PROBE_URL: &str = "https://www.gstatic.com/generate_204";
pub const PROBE_INTERVAL_S: u64 = 300;
pub const PROBE_TOLERANCE_MS: u64 = 50;

/// One connection as the engine serves it.
#[derive(Debug, Clone)]
pub struct Entrance {
    pub id: String,
    pub port: u16,
    pub servers: Vec<Server>,
    /// `auto` or a server's name.
    pub choice: String,
}

/// What the engine's local doors are locked with: the entrances' login, the Clash API's port and secret.
#[derive(Debug, Clone)]
pub struct Access {
    pub user: String,
    pub password: String,
    pub clash_port: u16,
    pub clash_secret: String,
}

pub fn auto_group(id: &str) -> String {
    format!("{id}-auto")
}

pub fn choice_group(id: &str) -> String {
    format!("{id}-choice")
}

pub fn server_group(id: &str, index: usize) -> String {
    format!("{id}-s{index}")
}

impl Entrance {
    /// The selector's member for the person's choice: the fastest, or the named server while it is still there.
    pub fn chosen_member(&self) -> String {
        match self.servers.iter().position(|server| server.name == self.choice) {
            Some(index) if self.choice != "auto" => server_group(&self.id, index),
            _ => auto_group(&self.id),
        }
    }

    /// The server a member stands for.
    pub fn server_of(&self, member: &str) -> Option<&Server> {
        let index: usize = member.strip_prefix(&format!("{}-s", self.id))?.parse().ok()?;
        self.servers.get(index)
    }
}

/// The groups of one connection.
pub fn groups(entrance: &Entrance) -> Vec<Value> {
    let id = &entrance.id;
    let mut entries = Vec::new();
    let members: Vec<String> = (0..entrance.servers.len()).map(|index| server_group(id, index)).collect();
    for (server, member) in entrance.servers.iter().zip(&members) {
        entries.push(json!({ "client_group": member, "client_chains": [server.hop.clone()] }));
    }
    let chains = |names: &[String]| -> Vec<Value> { names.iter().map(|name| json!({ "chain": [name] })).collect() };
    entries.push(json!({
        "client_group": auto_group(id),
        "type": "url-test",
        "url": PROBE_URL,
        "interval": PROBE_INTERVAL_S,
        "tolerance": PROBE_TOLERANCE_MS,
        "client_chains": chains(&members),
    }));
    let mut choices = vec![auto_group(id)];
    choices.extend(members.iter().cloned());
    entries.push(json!({
        "client_group": choice_group(id),
        "type": "selector",
        "default": entrance.chosen_member(),
        "client_chains": chains(&choices),
    }));
    entries
}

fn entrance(id: &str, port: u16, access: &Access) -> Value {
    json!({
        "address": format!("127.0.0.1:{port}"),
        "protocol": {
            "type": "mixed",
            "username": access.user,
            "password": access.password,
            "udp_enabled": false,
        },
        "rules": [{ "masks": ["0.0.0.0/0", "::/0"], "action": "allow", "client_chain": choice_group(id) }],
    })
}

/// The whole file: the Clash API, then each connection that has servers.
pub fn build(entrances: &[Entrance], access: &Access) -> Value {
    let mut entries = vec![json!({
        "clash_api_address": format!("127.0.0.1:{}", access.clash_port),
        "secret": access.clash_secret,
    })];
    for one in entrances.iter().filter(|one| !one.servers.is_empty()) {
        entries.extend(groups(one));
        entries.push(entrance(&one.id, one.port, access));
    }
    Value::Array(entries)
}

/// One server alone with an entrance, to find which of a list the engine refuses.
pub fn single(server: &Server, access: &Access) -> Value {
    let one = Entrance { id: "check".into(), port: 1, servers: vec![server.clone()], choice: "auto".into() };
    let mut entries = groups(&one);
    entries.push(entrance(&one.id, one.port, access));
    Value::Array(entries)
}

pub fn yaml(config: &Value) -> String {
    serde_yaml::to_string(config).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::parse_link;

    fn access() -> Access {
        Access { user: "u".into(), password: "p".into(), clash_port: 9999, clash_secret: "s".into() }
    }

    fn entrance_of(choice: &str) -> Entrance {
        Entrance {
            id: "c1".into(),
            port: 4000,
            servers: vec![
                parse_link("trojan://p@a.example:443#🇳🇱 A").unwrap(),
                parse_link("trojan://p@b.example:443#🇩🇪 B").unwrap(),
            ],
            choice: choice.into(),
        }
    }

    #[test]
    fn groups_and_the_entrance() {
        let config = build(&[entrance_of("auto")], &access());
        let entries = config.as_array().unwrap();
        assert_eq!(entries[0], json!({ "clash_api_address": "127.0.0.1:9999", "secret": "s" }));
        assert_eq!(entries[1]["client_group"], "c1-s0");
        assert_eq!(entries[1]["client_chains"][0]["address"], "a.example:443");
        assert_eq!(
            entries[3],
            json!({
                "client_group": "c1-auto", "type": "url-test", "url": PROBE_URL, "interval": 300, "tolerance": 50,
                "client_chains": [{ "chain": ["c1-s0"] }, { "chain": ["c1-s1"] }],
            })
        );
        assert_eq!(entries[4]["default"], "c1-auto");
        assert_eq!(
            entries[4]["client_chains"],
            json!([{ "chain": ["c1-auto"] }, { "chain": ["c1-s0"] }, { "chain": ["c1-s1"] }])
        );
        assert_eq!(entries[5]["address"], "127.0.0.1:4000");
        assert_eq!(entries[5]["rules"][0]["client_chain"], "c1-choice");
        assert!(yaml(&config).contains("client_group: c1-auto"));
    }

    #[test]
    fn a_chosen_server_and_one_that_left() {
        let chosen = entrance_of("🇩🇪 B");
        assert_eq!(chosen.chosen_member(), "c1-s1");
        assert_eq!(chosen.server_of("c1-s1").unwrap().name, "🇩🇪 B");
        assert_eq!(entrance_of("gone").chosen_member(), "c1-auto");
        let empty = Entrance { servers: vec![], ..entrance_of("auto") };
        assert_eq!(build(&[empty], &access()).as_array().unwrap().len(), 1);
    }
}
