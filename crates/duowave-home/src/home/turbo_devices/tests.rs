//! The home's devices on stand-in Турбо: no network, the helpers' list as the tests set it.
use std::sync::Mutex;

use super::*;
use crate::bus::providers::devices::Device;

/// Турбо as a test sets them, and the letters they got; the Mac's home folder when a test gives it one.
#[derive(Default)]
struct Stand {
    turbos: Mutex<Vec<TurboAt>>,
    letters: Mutex<Vec<(String, String, String, Value)>>,
    wakes_up: Mutex<bool>,
    mac_files: Mutex<Option<Arc<crate::bus::providers::files::Files>>>,
}

impl TurboSource for Stand {
    fn turbos(&self) -> Vec<TurboAt> {
        self.turbos.lock().unwrap().clone()
    }

    fn call(&self, turbo: &str, kind: &str, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        self.letters.lock().unwrap().push((turbo.into(), kind.into(), handle.into(), input.clone()));
        let answer = match handle {
            "keyboards" => json!([{ "name": format!("Keys of {turbo}"), "input": "in-1", "output": "out-1" }]),
            other => json!({ "did": other, "input": input }),
        };
        Box::pin(async move { Ok(answer) })
    }

    fn look(&self, _turbo: &str) -> BoxFuture<'static, bool> {
        let up = *self.wakes_up.lock().unwrap();
        Box::pin(async move { up })
    }

    /// The Mac's `/home/v1/files/call`: the letter on its own folders, as its route does.
    fn files(&self, turbo: &str, to: &str, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
        use crate::bus::provider::Provider;
        let files = self.mac_files.lock().unwrap().clone().filter(|_| turbo == "mac");
        let letter = crate::bus::Letter {
            id: 0,
            from: crate::bus::Address::new("gateway", "centre").unwrap(),
            to: crate::bus::Address::parse(to).unwrap(),
            handle: handle.into(),
            input,
            files: Vec::new(),
            trail: crate::bus::Trail::system(),
        };
        Box::pin(async move {
            match files {
                Some(files) => files.call(letter).await,
                None => Err(BusError::Failed("no folders there".into())),
            }
        })
    }

    fn media_base(&self, turbo: &str) -> Option<String> {
        (turbo == "mac").then(|| "http://mac.local:7780".to_string())
    }
}

fn midi(instances: &[(&str, &str)]) -> GivenKind {
    GivenKind {
        kind: "midi".into(),
        title: "MIDI-клавиатура".into(),
        nature: Nature::Physical,
        handles: vec![
            GivenHandle { name: "keyboards".into(), level: Level::See, title: None },
            GivenHandle { name: "notes".into(), level: Level::Listen, title: None },
            GivenHandle { name: "play".into(), level: Level::Call, title: None },
        ],
        instances: instances.iter().map(|(id, title)| Instance { id: (*id).into(), title: (*title).into() }).collect(),
    }
}

fn turbo(id: &str, name: &str, kinds: Vec<GivenKind>) -> TurboAt {
    TurboAt {
        id: id.into(),
        computer: format!("gw-{id}"),
        name: name.into(),
        cloud: false,
        up: true,
        home: true,
        kinds,
        files: false,
        macs: vec!["a4:83:e7:01:02:03".into()],
    }
}

fn home_with(turbos: Vec<TurboAt>) -> (Arc<HouseDevices>, Arc<Stand>) {
    let stand = Arc::new(Stand::default());
    *stand.turbos.lock().unwrap() = turbos;
    let house = HouseDevices::new(Arc::new(Devices::new()), Some(Arc::new(Db::memory())));
    house.set_source(stand.clone());
    house.set_waker(Arc::new(|macs: Vec<String>| Box::pin(async move { Ok(macs.len()) })));
    (house, stand)
}

#[test]
fn a_turbos_keyboard_is_the_homes_by_its_place() {
    let (house, _) = home_with(vec![turbo("mac", "Mac", vec![midi(&[("in-2", "Keystation 49")])])]);
    let places = house.places("midi");
    assert_eq!(places.len(), 1);
    assert_eq!((places[0].place.as_str(), places[0].state), ("mac", PlaceState::Ready));
    assert_eq!(places[0].instances, [Instance { id: "mac:in-2".into(), title: "Keystation 49".into() }]);
    assert_eq!(house.whereabouts("midi"), Whereabouts::Turbo("Mac".into()));
    assert!(house.knows("midi") && !house.knows("camera"));
    let view = house.view();
    assert_eq!((view.len(), view[0].kind.as_str(), view[0].title.as_str()), (1, "midi", "MIDI-клавиатура"));
}

#[test]
fn a_sleeping_or_absent_turbo_is_said_so_and_remembered() {
    let (house, stand) = home_with(vec![turbo("mac", "Mac", vec![midi(&[("in-2", "Keystation 49")])])]);
    house.remember();
    stand.turbos.lock().unwrap()[0].up = false;
    stand.turbos.lock().unwrap()[0].kinds.clear();
    let places = house.places("midi");
    assert_eq!(places[0].state, PlaceState::Asleep, "remembered while it sleeps");
    assert_eq!(places[0].instances[0].title, "Keystation 49");
    assert_eq!(house.whereabouts("midi"), Whereabouts::Asleep("Mac".into()));
    assert!(house.why_none("midi").contains("Mac") && house.why_none("midi").contains("спит"));
    assert_eq!(house.sleeping_with("midi"), Some(("mac".into(), "Mac".into())));
    assert!(house.knows("midi"), "a sleeping Mac's keyboard stays an address");
    // On the line again, nothing plugged in: «подключите».
    stand.turbos.lock().unwrap()[0].up = true;
    assert_eq!(house.places("midi")[0].state, PlaceState::Unplugged);
    assert!(house.why_none("midi").contains("подключите"));
}

#[test]
fn a_thing_at_hand_is_the_homes_only_at_home_and_data_from_afar() {
    let mut folders = midi(&[("photos", "Фото")]);
    folders.kind = "folders".into();
    folders.nature = Nature::Data;
    let mut away = turbo("mac", "Mac", vec![midi(&[("in-2", "Keystation 49")]), folders]);
    away.home = false;
    let (house, _) = home_with(vec![away]);
    assert_eq!(house.places("midi")[0].state, PlaceState::Away);
    assert!(house.why_none("midi").contains("не дома"));
    assert_eq!(house.places("folders")[0].state, PlaceState::Ready, "folders reach from afar (owner 20.11, 6)");
}

#[test]
fn no_turbo_and_never_seen_are_told_apart() {
    let (house, _) = home_with(Vec::new());
    assert_eq!(house.whereabouts("midi"), Whereabouts::NoTurbo);
    let (house, _) = home_with(vec![turbo("mac", "Mac", Vec::new())]);
    assert_eq!(house.whereabouts("midi"), Whereabouts::Unplugged("Mac".into()), "a Mac at home with nothing plugged");
    let mut asleep = turbo("mac", "Mac", Vec::new());
    asleep.up = false;
    let (house, _) = home_with(vec![asleep]);
    assert_eq!(house.whereabouts("midi"), Whereabouts::Never);
}

#[tokio::test]
async fn sight_goes_everywhere_and_an_act_to_one_place_said_once() {
    let (house, stand) = home_with(vec![
        turbo("mac", "Mac", vec![midi(&[("in-1", "Keys")])]),
        turbo("pc", "ПК", vec![midi(&[("in-1", "Keys")])]),
    ]);
    // Seeing: both places, each item marked.
    let seen = house.letter("midi", "keyboards", json!({})).await.unwrap();
    let seen = seen.as_array().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!((seen[0]["input"].as_str(), seen[0]["place"].as_str()), (Some("mac:in-1"), Some("mac")));
    assert_eq!(seen[1]["output"], "pc:out-1");
    // The notes' channel is the panel's own.
    assert_eq!(house.letter("midi", "notes", json!({})).await.unwrap()["channel"], "/api/v1/midi/ws");
    // Acting: one place, the choice said the first time.
    let first = house.letter("midi", "play", json!({ "notes": [], "ear": "kitchen" })).await.unwrap();
    let said = first["said"].as_str().unwrap();
    assert!(said.contains("Mac") && said.contains("ПК"), "{said}");
    assert_eq!(stand.letters.lock().unwrap().last().unwrap().0, "mac");
    assert!(stand.letters.lock().unwrap().last().unwrap().3.get("ear").is_none(), "the ear stays at the centre");
    let again = house.letter("midi", "play", json!({ "notes": [], "ear": "kitchen" })).await.unwrap();
    assert!(again.get("said").is_none(), "said once: {again}");
    // «На ПК»: the choice changes, said by the person — not said back.
    house.set_choice("midi", "kitchen", "пк").unwrap();
    let chosen = house.letter("midi", "play", json!({ "notes": [], "ear": "kitchen" })).await.unwrap();
    assert!(chosen.get("said").is_none());
    assert_eq!(stand.letters.lock().unwrap().last().unwrap().0, "pc");
    // Another room chooses on its own: the house's last use leans it to the ПК.
    let nursery = house.choose("midi", "nursery").unwrap();
    assert_eq!(nursery.place, "pc");
    // A device named by its id goes where it is, without a choice; its id there is bare.
    house.letter("midi", "play", json!({ "notes": [], "device": "mac:out-1" })).await.unwrap();
    let letter = stand.letters.lock().unwrap().last().unwrap().clone();
    assert_eq!((letter.0.as_str(), letter.3["device"].as_str()), ("mac", Some("out-1")));
}

#[tokio::test]
async fn a_letter_to_a_sleeping_place_says_where_and_why() {
    let mut asleep = turbo("mac", "Mac", vec![midi(&[("in-1", "Keys")])]);
    let (house, stand) = home_with(vec![asleep.clone()]);
    house.remember();
    asleep.up = false;
    asleep.kinds.clear();
    *stand.turbos.lock().unwrap() = vec![asleep];
    let error = house.letter("midi", "play", json!({ "notes": [] })).await.unwrap_err();
    assert!(error.to_string().contains("спит"), "{error}");
    assert!(stand.letters.lock().unwrap().is_empty(), "nothing sent to a sleeping Mac");
}

#[tokio::test]
async fn a_wake_waits_for_the_turbo_and_says_when_it_did_not_come() {
    let mut asleep = turbo("mac", "Mac", vec![midi(&[("in-1", "Keys")])]);
    asleep.up = false;
    let (house, stand) = home_with(vec![asleep]);
    *stand.wakes_up.lock().unwrap() = true;
    assert_eq!(house.wake("mac", WAKE_WAIT).await, Ok(true));
    let (house, _) = house_without_macs();
    assert!(house.wake("mac", WAKE_WAIT).await.is_err(), "no address — no wake, said in words");
}

/// Several Турбо (0.18.2): the station's door `home-computer` passes to another computer when the first one's entry
/// was removed and another joins. What was seen at the Mac is not said of the Windows, the Mac's addresses never wake
/// it, and a choice of the Mac is no choice of it.
#[tokio::test]
async fn an_entry_taken_by_another_computer_keeps_nothing_of_the_one_before() {
    let mut mac = turbo("home-computer", "Mac", vec![midi(&[("in-2", "Keystation 49")])]);
    mac.computer = "gw-mac".into();
    let pc = turbo("turbo-gw-pc", "ПК", vec![midi(&[("in-1", "Keys")])]);
    let (house, stand) = home_with(vec![mac.clone(), pc.clone()]);
    house.remember();
    house.set_choice("midi", "kitchen", "mac").unwrap();
    assert_eq!(house.choose("midi", "kitchen").unwrap().place, "home-computer");
    // The Mac sleeps: remembered, woken by its own addresses.
    mac.up = false;
    mac.kinds.clear();
    mac.macs.clear();
    *stand.turbos.lock().unwrap() = vec![mac.clone(), pc.clone()];
    assert_eq!(house.places("midi")[0].instances[0].title, "Keystation 49");
    assert_eq!(house.macs_of("home-computer"), ["a4:83:e7:01:02:03"]);
    // The door now is the Windows' (it said nothing yet of its devices or its addresses).
    let mut windows = turbo("home-computer", "Windows", Vec::new());
    windows.computer = "gw-win".into();
    windows.up = false;
    windows.macs.clear();
    *stand.turbos.lock().unwrap() = vec![windows, pc];
    assert!(house.places("midi").iter().all(|place| place.place != "home-computer"), "no Mac's keyboard at Windows");
    assert!(house.macs_of("home-computer").is_empty(), "the Mac's addresses do not wake Windows");
    assert_eq!(house.sleeping_turbo(), None);
    let chosen = house.choose("midi", "kitchen").unwrap();
    assert_eq!(chosen.place, "turbo-gw-pc", "the only place ready");
    // A removed entry's kinds are not the home's.
    *stand.turbos.lock().unwrap() = Vec::new();
    assert!(!house.knows("midi"));
}

fn house_without_macs() -> (Arc<HouseDevices>, Arc<Stand>) {
    let mut asleep = turbo("mac", "Mac", Vec::new());
    asleep.up = false;
    asleep.macs.clear();
    home_with(vec![asleep])
}

#[test]
fn a_computer_gives_its_home_what_it_shares_and_a_sensitive_kind_only_ticked() {
    struct Camera;
    impl Device for Camera {
        fn name(&self) -> &'static str {
            "camera"
        }
        fn title(&self) -> String {
            "Камера".into()
        }
        fn handles(&self) -> Vec<Handle> {
            vec![Handle::new("snap", Level::Call)]
        }
        fn sensitive(&self) -> bool {
            true
        }
        fn call(&self, _handle: &str, _input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
            Box::pin(async { Ok(Value::Null) })
        }
    }
    struct Pedal;
    impl Device for Pedal {
        fn name(&self) -> &'static str {
            "pedal"
        }
        fn title(&self) -> String {
            "Педаль".into()
        }
        fn handles(&self) -> Vec<Handle> {
            vec![Handle::new("pressed", Level::Listen)]
        }
        fn instances(&self) -> Vec<Instance> {
            vec![Instance { id: "p1".into(), title: "AirTurn".into() }]
        }
        fn call(&self, _handle: &str, _input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
            Box::pin(async { Ok(Value::Null) })
        }
    }
    let local = Arc::new(Devices::new());
    local.add(Arc::new(Camera));
    local.add(Arc::new(Pedal));
    let house = HouseDevices::new(local, None);
    let ticked = Arc::new(Mutex::new(TurboShares::default()));
    {
        let ticked = ticked.clone();
        house.set_shares(Arc::new(move || ticked.lock().unwrap().clone()));
    }
    let kinds = |house: &HouseDevices| house.given().into_iter().map(|given| given.kind).collect::<Vec<_>>();
    assert_eq!(kinds(&house), ["pedal"], "the camera is off until ticked (owner 20.11, 3)");
    assert_eq!(house.given()[0].instances[0].title, "AirTurn");
    ticked.lock().unwrap().kinds.insert("camera".into(), true);
    ticked.lock().unwrap().kinds.insert("pedal".into(), false);
    assert_eq!(kinds(&house), ["camera"]);
}

#[test]
fn ids_name_their_place() {
    let places = vec!["mac".to_string(), "home-computer".to_string()];
    assert_eq!(home_id(HERE, "in-1"), "in-1");
    assert_eq!(home_id("mac", "in-1"), "mac:in-1");
    assert_eq!(split_id("mac:in-1", &places), ("mac", "in-1"));
    assert_eq!(split_id("home-computer:out-2", &places), ("home-computer", "out-2"));
    assert_eq!(split_id("in-1", &places), (HERE, "in-1"));
    assert_eq!(split_id("other:in-1", &places), (HERE, "other:in-1"), "a prefix no Турбо has is this gateway's id");
}

#[test]
fn the_home_kinds_go_on_the_bus_by_name() {
    let (house, _) = home_with(vec![turbo("mac", "Mac", vec![midi(&[("in-1", "Keys")])])]);
    let devices = Devices::new();
    register(&devices, &house);
    let names: Vec<&str> = devices.list().iter().map(|device| device.name()).collect();
    assert_eq!(names, ["midi"], "only what the home knows can be reached: {names:?}");
    let midi = devices.get("midi").unwrap();
    assert_eq!(midi.instances()[0].id, "mac:in-1");
    assert!(midi.handles().iter().any(|handle| handle.name == "play"));
}

#[tokio::test]
async fn the_brain_sees_where_things_are_uses_them_and_wakes_a_sleeping_computer() {
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::roles::Role;
    use crate::dialog::connectors::{Connector, ToolContext};
    let (house, stand) = home_with(Vec::new());
    let connector = super::tools::DevicesConnector::without_bus(&house);
    assert!(connector.tools().is_empty(), "nothing plugged in anywhere — no tools");
    *stand.turbos.lock().unwrap() = vec![turbo("mac", "Mac", vec![midi(&[("in-1", "Keys")])])];
    let tools = connector.tools();
    let tool = |name: &str| tools.iter().find(|tool| tool.tool.name == name).unwrap();
    let context = || ToolContext {
        principal: household_principal(Role::Member, None),
        cancel: tokio_util::sync::CancellationToken::new(),
        ear: None,
        origin: Some(crate::talk::Origin::station("kitchen", true)),
    };
    let input = |value: Value| value.as_object().cloned().unwrap();
    let listed = (tool("home_devices").run)(input(json!({})), context()).await.unwrap();
    assert!(listed.contains("\"computer\":\"Mac\"") && listed.contains("Keys"), "{listed}");
    let played = (tool("home_device_use").run)(
        input(json!({ "kind": "midi", "action": "play", "input": { "notes": [[60, 90, 0, 100]] } })),
        context(),
    )
    .await
    .unwrap();
    assert!(played.contains("\"did\":\"play\""), "{played}");
    let letter = stand.letters.lock().unwrap().last().unwrap().clone();
    assert_eq!((letter.0.as_str(), letter.2.as_str()), ("mac", "play"));
    // A sleeping Mac: said so, and woken on the model's call after the person's «да».
    stand.turbos.lock().unwrap()[0].up = false;
    house.remember();
    let listed = (tool("home_devices").run)(input(json!({})), context()).await.unwrap();
    assert!(listed.contains("\"canWake\":true"), "{listed}");
    *stand.wakes_up.lock().unwrap() = true;
    let woke = (tool("wake_computer").run)(input(json!({})), context()).await.unwrap();
    assert!(woke.contains("\"awake\":true"), "{woke}");
}

/// A home folder of its own: a computer's (`~/DuoWave`) or a station's (`DATA_DIR/files/home`).
fn folders(station: bool) -> (Arc<crate::bus::providers::files::Files>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let places =
        crate::bus::providers::files::Places::new(&dir.path().join("data"), &dir.path().join("content"), station);
    std::fs::create_dir_all(dir.path().join("content/music")).unwrap();
    places.prepare();
    (Arc::new(crate::bus::providers::files::Files::new(places, Arc::new(|_| 64))), dir)
}

fn letter(to: &str, handle: &str, input: Value) -> crate::bus::Letter {
    crate::bus::Letter {
        id: 1,
        from: crate::bus::Address::app("folders"),
        to: crate::bus::Address::parse(to).unwrap(),
        handle: handle.into(),
        input,
        files: Vec::new(),
        trail: crate::bus::Trail::system(),
    }
}

/// The owner's decision 1 for folders: one «Фото» of the home — the station's and the Mac's — seen together, read where
/// a file is, a new file on a station put at the Mac; a Mac's file shown by its link through the centre.
#[tokio::test]
async fn the_homes_folders_are_seen_everywhere_read_where_a_file_is_and_put_at_one_place() {
    use crate::bus::provider::Provider;
    let (centre, _centre_dir) = folders(true);
    let (mac, _mac_dir) = folders(false);
    let mut at_mac = turbo("mac", "Mac", Vec::new());
    at_mac.files = true;
    let (house, stand) = home_with(vec![at_mac]);
    *stand.mac_files.lock().unwrap() = Some(mac.clone());
    let home = super::files::HouseFiles::new(centre.clone(), &house, Arc::new(|| true));
    let write = |text: &str| json!({ "text": text });
    centre.call(letter("files/home/Photos/дом.txt", "write", write("дом"))).await.unwrap();
    mac.call(letter("files/home/Photos/море.txt", "write", write("море"))).await.unwrap();

    let listed = home.call(letter("files/home/Photos", "list", json!({}))).await.unwrap();
    let items = listed["items"].as_array().unwrap();
    let seen: Vec<(&str, &str)> =
        items.iter().map(|item| (item["name"].as_str().unwrap(), item["place"].as_str().unwrap())).collect();
    assert_eq!(seen, [("дом.txt", ""), ("море.txt", "mac")], "one list, each with its place");
    assert_eq!(listed["total"], 2);
    assert_eq!(items[1]["placeName"], "Mac");

    let read = home.call(letter("files/home/Photos/море.txt", "read", json!({ "as": "text" }))).await.unwrap();
    assert_eq!(read["text"], "море", "read where it is: at the Mac");
    let read = home.call(letter("files/home/Photos/дом.txt", "read", json!({ "as": "text" }))).await.unwrap();
    assert_eq!(read["text"], "дом", "here when here has it");

    home.call(letter("files/home/Photos/новое.txt", "write", write("новое"))).await.unwrap();
    let there = mac.call(letter("files/home/Photos/новое.txt", "read", json!({ "as": "text" }))).await.unwrap();
    assert_eq!(there["text"], "новое", "a station puts a new file at its Турбо");
    assert!(centre.call(letter("files/home/Photos/новое.txt", "stat", json!({}))).await.is_err());

    // An app's own folder stays this gateway's.
    home.call(letter("files/app/folders/notes.txt", "write", write("свои"))).await.unwrap();
    assert!(centre.call(letter("files/app/folders/notes.txt", "stat", json!({}))).await.is_ok());

    // A Mac's file by its link: the key named with its place, fetched at the Mac's own address.
    let link = super::files::keyed_for_tests(json!({ "file": "abc", "key": "abc", "title": "море" }), "mac");
    assert_eq!((link["file"].as_str(), link["place"].as_str()), (Some("turbo.mac.abc"), Some("mac")));
    assert_eq!(house.media_url("mac", "abc").as_deref(), Some("http://mac.local:7780/media/v/abc"));
    assert_eq!(house.media_url("pc", "abc"), None);
}

/// «Покажи фото с моря на экране»: a Mac's file is shown on the Mac's screen, with the key the Mac knows; files of two
/// computers at once are not.
#[tokio::test]
async fn a_file_is_shown_on_the_screen_of_the_computer_it_is_at() {
    let screen = |instances: &[(&str, &str)]| GivenKind {
        kind: "screen".into(),
        title: "Экран компьютера".into(),
        nature: Nature::Physical,
        handles: vec![GivenHandle { name: "open".into(), level: Level::Call, title: None }],
        instances: instances.iter().map(|(id, title)| Instance { id: (*id).into(), title: (*title).into() }).collect(),
    };
    let (house, stand) = home_with(vec![
        turbo("pc", "ПК", vec![screen(&[("screen", "Экран")])]),
        turbo("mac", "Mac", vec![screen(&[("screen", "Экран")])]),
    ]);
    let items = json!({ "items": [{ "kind": "image", "title": "Море", "file": "turbo.mac.k1" }] });
    house.letter("screen", "open", items).await.unwrap();
    let letter = stand.letters.lock().unwrap().last().unwrap().clone();
    assert_eq!((letter.0.as_str(), letter.3["items"][0]["file"].as_str()), ("mac", Some("k1")), "the Mac's own key");
    let mixed = json!({ "items": [{ "file": "turbo.mac.k1" }, { "file": "turbo.pc.k2" }] });
    assert!(house.letter("screen", "open", mixed).await.is_err(), "two computers' files at once");
}

/// «Что на флешке?» on the station: a drive plugged into the Mac is a folder of the home, read at the Mac, read only.
#[tokio::test]
async fn a_turbos_flash_drive_is_read_there_and_only_read() {
    use crate::bus::provider::Provider;
    let (centre, _centre_dir) = folders(true);
    let (mac, mac_dir) = folders(false);
    let drive = mac_dir.path().join("KINGSTON");
    std::fs::create_dir_all(&drive).unwrap();
    std::fs::write(drive.join("отпуск.txt"), "море").unwrap();
    mac.places().set_attached(vec![crate::bus::providers::files::AttachedPlace {
        name: "KINGSTON".into(),
        path: drive,
        write: false,
    }]);
    let storage = GivenKind {
        kind: "storage".into(),
        title: "Флешки и диски".into(),
        nature: Nature::Physical,
        handles: vec![GivenHandle { name: "drives".into(), level: Level::See, title: None }],
        instances: vec![Instance { id: "KINGSTON".into(), title: "KINGSTON".into() }],
    };
    let mut at_mac = turbo("mac", "Mac", vec![storage]);
    at_mac.files = true;
    let (house, stand) = home_with(vec![at_mac]);
    *stand.mac_files.lock().unwrap() = Some(mac.clone());
    let home = super::files::HouseFiles::new(centre.clone(), &house, Arc::new(|| true));
    let drive = crate::bus::Address::parse("files/home/Connected/KINGSTON").unwrap();
    assert!(home.participant(&drive).is_some(), "the centre knows the Mac's drive as a folder");
    assert!(home.handles(&drive).iter().all(|handle| handle.level != Level::Edit), "read only");
    let listed = home.call(letter("files/home/Connected/KINGSTON", "list", json!({}))).await.unwrap();
    assert_eq!(listed["items"][0]["name"], "отпуск.txt");
    let read =
        home.call(letter("files/home/Connected/KINGSTON/отпуск.txt", "read", json!({ "as": "text" }))).await.unwrap();
    assert_eq!(read["text"], "море");
    let put = home.call(letter("files/home/Connected/KINGSTON/новое.txt", "write", json!({ "text": "x" }))).await;
    assert!(put.is_err(), "nothing put on another computer's drive from here");
}
