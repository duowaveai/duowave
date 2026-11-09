//! Who serves each capability, in order of preference: ROUTE_DEFAULT for all, ROUTES for exceptions
//! (`radio=yandex;timers=own`). A route is `own`, `yandex`, `off` or a list (`own,yandex`: ours, and Yandex when we
//! have nothing). An empty route is off: "not yet" and a journal entry.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::capabilities::{is_capability, CapabilityName, CAPABILITY_NAMES};
use super::js;

/// own: our implementation; yandex: the request goes to Yandex through our session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteStep {
    Own,
    Yandex,
}

impl RouteStep {
    pub fn as_str(self) -> &'static str {
        match self {
            RouteStep::Own => "own",
            RouteStep::Yandex => "yandex",
        }
    }
}

/// A describe() map back to ROUTES syntax (`radio=yandex;timers=own`): what a stored routes setting keeps.
pub fn serialize_routes<'a>(routes: impl IntoIterator<Item = (&'a String, &'a String)>) -> String {
    routes.into_iter().map(|(name, route)| format!("{name}={route}")).collect::<Vec<_>>().join(";")
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteTable {
    fallback: Vec<RouteStep>,
    overrides: BTreeMap<CapabilityName, Vec<RouteStep>>,
    /// Problems found while parsing, for the startup log.
    pub problems: Vec<String>,
}

impl RouteTable {
    pub fn parse(default_route: &str, routes: &str) -> RouteTable {
        let mut problems = Vec::new();
        let fallback = parse_route(default_route, "ROUTE_DEFAULT", &mut problems)
            .unwrap_or(vec![RouteStep::Own, RouteStep::Yandex]);
        let mut overrides = BTreeMap::new();
        for entry in routes.split([';', '\n']) {
            if js::trim(entry).is_empty() {
                continue;
            }
            let mut parts = entry.split('=').map(js::trim);
            let (name, value) = (parts.next().unwrap_or(""), parts.next());
            let Some(value) = value.filter(|_| !name.is_empty()) else {
                problems.push(format!("ROUTES: \"{}\" is not name=route", js::trim(entry)));
                continue;
            };
            let Some(capability) = is_capability(name) else {
                let known: Vec<&str> = CAPABILITY_NAMES.iter().map(|name| name.as_str()).collect();
                problems.push(format!("ROUTES: unknown capability \"{name}\" (known: {})", known.join(", ")));
                continue;
            };
            if let Some(route) = parse_route(value, &format!("ROUTES {name}"), &mut problems) {
                overrides.insert(capability, route);
            }
        }
        RouteTable { fallback, overrides, problems }
    }

    pub fn route(&self, capability: CapabilityName) -> &[RouteStep] {
        self.overrides.get(&capability).unwrap_or(&self.fallback)
    }

    /// A capability's route as written in the settings: `own,yandex`, `off`.
    pub fn describe_one(&self, capability: CapabilityName) -> String {
        let steps: Vec<&str> = self.route(capability).iter().map(|step| step.as_str()).collect();
        if steps.is_empty() {
            "off".into()
        } else {
            steps.join(",")
        }
    }

    /// Every capability's route as written in the settings, in the catalog's order.
    pub fn describe(&self) -> BTreeMap<CapabilityName, String> {
        CAPABILITY_NAMES.iter().map(|name| (*name, self.describe_one(*name))).collect()
    }

    /// describe() keyed by the capabilities' names: the stored setting and the event carry it so.
    pub fn describe_by_name(&self) -> BTreeMap<String, String> {
        self.describe().into_iter().map(|(name, route)| (name.as_str().to_string(), route)).collect()
    }
}

fn parse_route(value: &str, place: &str, problems: &mut Vec<String>) -> Option<Vec<RouteStep>> {
    let steps: Vec<&str> = value.split(',').map(js::trim).filter(|step| !step.is_empty()).collect();
    if steps == ["off"] {
        return Some(Vec::new());
    }
    let mut route = Vec::new();
    for step in steps {
        let step = match step {
            "own" => RouteStep::Own,
            "yandex" => RouteStep::Yandex,
            _ => {
                problems.push(format!("{place}: \"{step}\" is not own, yandex or off"));
                return None;
            }
        };
        if !route.contains(&step) {
            route.push(step);
        }
    }
    Some(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default_route_and_the_exceptions() {
        let table = RouteTable::parse("own,yandex", "radio=yandex; timers=off;calls=own");
        assert_eq!(table.route(CapabilityName::Chat), [RouteStep::Own, RouteStep::Yandex]);
        assert_eq!(table.route(CapabilityName::Radio), [RouteStep::Yandex]);
        assert!(table.route(CapabilityName::Timers).is_empty());
        assert_eq!(table.describe()[&CapabilityName::Timers], "off");
        assert!(table.problems.is_empty());
    }

    #[test]
    fn reports_what_it_cannot_read_and_keeps_the_rest() {
        let table = RouteTable::parse("own,yandex", "radio=everywhere;nonsense=own;music=yandex");
        assert_eq!(table.route(CapabilityName::Music), [RouteStep::Yandex]);
        assert_eq!(table.route(CapabilityName::Radio), [RouteStep::Own, RouteStep::Yandex]);
        assert_eq!(table.problems.len(), 2);
        assert_eq!(table.problems[0], "ROUTES radio: \"everywhere\" is not own, yandex or off");
        assert!(table.problems[1].starts_with("ROUTES: unknown capability \"nonsense\" (known: chat, volume,"));
        let broken = RouteTable::parse("everywhere", "radio;=own");
        assert_eq!(broken.route(CapabilityName::Chat), [RouteStep::Own, RouteStep::Yandex]);
        assert_eq!(
            broken.problems,
            [
                "ROUTE_DEFAULT: \"everywhere\" is not own, yandex or off",
                "ROUTES: \"radio\" is not name=route",
                "ROUTES: \"=own\" is not name=route",
            ]
        );
    }

    #[test]
    fn writes_the_routes_back_as_routes_syntax() {
        let table = RouteTable::parse("own", "radio=yandex,own,yandex");
        let described = table.describe_by_name();
        assert_eq!(described["radio"], "yandex,own");
        let again = RouteTable::parse("own,yandex", &serialize_routes(&described));
        assert_eq!(again.describe(), table.describe());
        assert_eq!(serde_json::to_value(table.describe()).unwrap()["chat"], "own");
    }
}
