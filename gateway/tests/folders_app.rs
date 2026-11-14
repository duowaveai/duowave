//! The built-in «Папки» (packages/folders/backend.js) in the gateway's own QuickJS, walking a real home folder: its
//! `host.files` and `host.bus` reach the gateway's folders' provider (src/bus/providers/files) on a temporary home
//! folder, as the household's voice — the bus's own rules are tested in src/bus. «что в папке Фото» → «открой
//! Отпуск» → «дальше» → «назад», the card following; «прочитай»; «найди»; «запиши»; a drive offered and opened by
//! «да»; names never said aloud without `speak`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use duowave::bus::providers::devices::Devices;
use duowave::bus::providers::files::drives::{Drive, Drives};
use duowave::bus::providers::files::rules::{Change, Folders};
use duowave::bus::providers::files::{Files, Places};
use duowave::bus::{Address, Letter, Provider, Trail};
use duowave::core::identity::principal::household_principal;
use duowave::core::identity::roles::Role;
use duowave::plugins::quickjs::QuickJs;
use duowave::plugins::runtime::{CallKind, HostApi, PluginCode, PluginInstance, PluginRuntime};
use duowave::screen::device::ScreenDevice;
use duowave::screen::Screen;
use duowave::storage::Db;
use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

struct Host {
    files: Arc<Files>,
    /// The computer's screen as `device/screen` (src/screen).
    devices: Arc<Devices>,
    kv: Mutex<Map<String, Value>>,
}

impl Host {
    fn letter(&self, to: Address, handle: &str, input: Value) -> Letter {
        Letter {
            id: 1,
            from: Address::app("folders"),
            to,
            handle: handle.into(),
            input,
            files: vec![],
            trail: Trail::principal(&household_principal(Role::Member, None)),
        }
    }
}

impl HostApi for Host {
    fn fetch(&self, _request: Value) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err("no network".to_string()) })
    }
    fn kv_get(&self, key: &str) -> Result<Option<Value>, String> {
        Ok(self.kv.lock().unwrap().get(key).cloned())
    }
    fn kv_set(&self, key: &str, value: Value) -> Result<(), String> {
        self.kv.lock().unwrap().insert(key.into(), value);
        Ok(())
    }
    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.kv.lock().unwrap().remove(key);
        Ok(())
    }
    fn kv_keys(&self, prefix: &str) -> Result<Vec<String>, String> {
        Ok(self.kv.lock().unwrap().keys().filter(|key| key.starts_with(prefix)).cloned().collect())
    }
    fn say(&self, _text: String, _device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        Box::pin(async { Ok(false) })
    }
    fn settings(&self) -> Map<String, Value> {
        Map::new()
    }
    fn log(&self, _level: &str, _message: &str) {}

    /// `folders.*` and `bus.*` as the gateway's bus resolves them (src/bus/mod.rs, `letter`), straight to the
    /// folders' provider: the rights are the bus's and are tested there.
    fn call(&self, name: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let at = |key: &str| {
            let given = field(key);
            if given.starts_with("files/") {
                Address::parse(&given)
            } else {
                Address::parse(&format!("files/app/folders/{}", given.trim_matches('/')))
            }
        };
        let letter = match name {
            "bus.list" => {
                let seen: Vec<Value> = self
                    .files
                    .participants()
                    .into_iter()
                    .map(|participant| json!({ "address": participant.address, "title": participant.title, "handles": [] }))
                    .collect();
                return Box::pin(async move { Ok(Value::Array(seen)) });
            }
            "bus.call" => {
                let to = Address::parse(&field("to")).unwrap();
                self.letter(to, &field("handle"), input.get("input").cloned().unwrap_or(json!({})))
            }
            folder if folder.starts_with("folders.") => {
                let handle = folder.trim_start_matches("folders.");
                let mut carried = input.clone();
                if handle == "move" || handle == "copy" {
                    carried = json!({ "to": at("to").unwrap().to_string() });
                }
                match at("at") {
                    Ok(to) => self.letter(to, handle, carried),
                    Err(error) => return Box::pin(async move { Err(error) }),
                }
            }
            other => {
                let other = other.to_string();
                return Box::pin(async move { Err(format!("нет функции {other}")) });
            }
        };
        let answer = if letter.to.kind() == "device" { self.devices.call(letter) } else { self.files.call(letter) };
        Box::pin(async move { answer.await.map_err(|error| error.to_string()) })
    }
}

fn package_file(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages/folders").join(name)).unwrap()
}

struct Home {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    folders: Arc<Folders>,
    drives: Arc<Drives>,
    screen: Arc<Screen>,
    plugin: Arc<dyn PluginInstance>,
    host: Arc<Host>,
}

/// A home folder with photos, recipes and music; the voice is ours (`speak`) unless `yandex`.
async fn home(yandex: bool) -> Home {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("DuoWave");
    let places =
        Places::new(&dir.path().join("data"), &dir.path().join("content"), false).with_home(Some(root.clone()));
    places.prepare();
    std::fs::create_dir_all(root.join("Фото/Отпуск 2025")).unwrap();
    for n in 1..=3 {
        image::RgbImage::from_pixel(64, 48, image::Rgb([n * 40, 90, 120]))
            .save(root.join(format!("Фото/Отпуск 2025/{n}.png")))
            .unwrap();
    }
    std::fs::create_dir_all(root.join("Рецепты/Супы")).unwrap();
    std::fs::write(root.join("Рецепты/Супы/Борщ украинский.txt"), "Свёкла, капуста, картошка.\n\nВарить час.").unwrap();
    std::fs::write(root.join("Рецепты/Щи.txt"), "Капуста.").unwrap();
    std::fs::write(root.join("Видео/Море.mp4"), b"not really a video").unwrap();
    std::fs::write(root.join("Документы/Голос.wav"), b"not really a sound").unwrap();
    image::RgbImage::from_pixel(32, 24, image::Rgb([200, 200, 200]))
        .save(root.join("Картинки/Screenshot 2026-11-13 at 16.01.11.png"))
        .unwrap();
    let files = Arc::new(Files::new(places.clone(), Arc::new(|_| 64)));
    let folders = Folders::new(Arc::new(Db::memory()), places, None);
    files.set_folders(folders.clone());
    files.set_leaving(Arc::new(move || !yandex), Arc::new(|| false));
    let drives = Drives::new(folders.clone(), None);
    files.set_drives(drives.clone());
    let screen = Screen::new(None);
    screen.set_available(true);
    let devices = Arc::new(Devices::new());
    devices.add(Arc::new(ScreenDevice::new(screen.clone())));
    let host = Arc::new(Host { files: files.clone(), devices, kv: Mutex::new(Map::new()) });
    let manifest: Value = serde_json::from_str(&package_file("manifest.json")).unwrap();
    let code = PluginCode {
        id: "folders".into(),
        version: manifest["version"].as_str().unwrap().into(),
        source: package_file("backend.js"),
        memory_mb: manifest["budget"]["memoryMb"].as_u64().unwrap(),
        ..PluginCode::default()
    };
    let plugin = QuickJs.start(code, host.clone() as Arc<dyn HostApi>).await.expect("«Папки» load");
    Home { _dir: dir, root, folders, drives, screen, plugin, host }
}

const REQUEST: fn() -> Value =
    || json!({ "text": "", "device": "kitchen", "timezone": "UTC", "capability": "plugins" });

async fn phrase(home: &Home, handler: &str, mut slots: Value) -> Value {
    slots["request"] = REQUEST();
    home.plugin
        .call(handler, slots.clone(), CallKind::Voice)
        .await
        .unwrap_or_else(|error| panic!("{handler} {slots}: {error}"))
}

async fn said(home: &Home, words: &str) -> Value {
    let input = json!({ "text": words, "request": REQUEST() });
    home.plugin.call("walk", input, CallKind::Voice).await.unwrap_or_else(|error| panic!("«{words}»: {error}"))
}

fn speech(answer: &Value) -> String {
    answer.get("say").and_then(Value::as_str).or_else(|| answer.as_str()).unwrap_or_default().to_string()
}

async fn card(home: &Home) -> Value {
    home.plugin.call("card", json!({}), CallKind::Background).await.unwrap()
}

#[tokio::test]
async fn a_walk_by_voice_through_the_photos_with_the_card_following() {
    let home = home(false).await;
    let listed = phrase(&home, "openFolder", json!({ "name": "фото" })).await;
    assert_eq!(speech(&listed), "В «Фото» 1 папка: Отпуск 2025. Что открыть?");
    assert_eq!(listed["continue"]["handler"], "walk");
    let opened = said(&home, "открой отпуск").await;
    assert!(speech(&opened).starts_with("В «Отпуск 2025» 3 файла: 1, 2, 3"), "{opened}");
    let first = said(&home, "номер один").await;
    assert_eq!(speech(&first), "Показываю «1».");
    let shown = card(&home).await;
    assert_eq!(shown["picture"]["address"], "files/home/Фото/Отпуск 2025/1.png");
    assert_eq!(shown["cap"], "Фото › Отпуск 2025 › 1.png · 1 из 3");
    assert_eq!(speech(&said(&home, "следующая").await), "Показываю «2».");
    assert_eq!(card(&home).await["picture"]["address"], "files/home/Фото/Отпуск 2025/2.png");
    assert_eq!(speech(&said(&home, "предыдущая").await), "Показываю «1».");
    let back = said(&home, "назад").await;
    assert!(speech(&back).starts_with("В «Отпуск 2025» 3 файла"), "{back}");
    let up = said(&home, "назад").await;
    assert_eq!(speech(&up), "В «Фото» 1 папка: Отпуск 2025. Что открыть?");
    assert_eq!(said(&home, "хватит").await, json!("Хорошо."));
    // The card's own buttons walk the same way.
    let tapped = home
        .plugin
        .call("cardOpen", json!({ "address": "files/home/Фото/Отпуск 2025" }), CallKind::Background)
        .await
        .unwrap();
    assert_eq!(tapped["rows"].as_array().unwrap().len(), 3);
}

/// Live 14.11: after «какие у меня папки» everything said goes to the conversation (`walk`), not to the app's phrases —
/// «открой фото», «что в папке фото» and «Stop.» (the recognizer's Latin) came back «Не поняла».
#[tokio::test]
async fn the_apps_own_phrases_work_inside_its_conversation() {
    let home = home(false).await;
    let listed = phrase(&home, "folders", json!({})).await;
    assert!(speech(&listed).starts_with("Папки: "), "{listed}");
    assert_eq!(speech(&said(&home, "Открой фото.").await), "В «Фото» 1 папка: Отпуск 2025. Что открыть?");
    // Another home folder, said from inside «Фото».
    let other = said(&home, "Что в папке рецепты?").await;
    assert_eq!(speech(&other), "В «Рецепты» 1 папка и 1 файл: Супы, Щи. Что открыть?");
    assert_eq!(speech(&said(&home, "Что в папке фото?").await), "В «Фото» 1 папка: Отпуск 2025. Что открыть?");
    let missing = said(&home, "открой папку отчёты").await;
    assert_eq!(speech(&missing), "Папки «отчеты» нет. Скажите «какие у меня папки».");
    assert_eq!(missing["continue"]["handler"], "walk");
    assert_eq!(speech(&said(&home, "покажи фото из отпуска").await), "В «Отпуск 2025» 3 фото. Показываю первое.");
    // «Фото» before a name is the kind of thing, not the folder: «открой фото 2» is the second photo here.
    assert_eq!(speech(&said(&home, "открой фото 2").await), "Показываю «2».");
    assert!(speech(&said(&home, "Какие у меня папки?").await).starts_with("Папки: "));
    assert_eq!(said(&home, "Stop.").await, json!("Хорошо."));
}

fn titles(home: &Home) -> Vec<(String, bool)> {
    home.screen.windows().into_iter().map(|window| (window.title, window.kept)).collect()
}

/// 14.11, the owner: a photo or a video opens a window of its own on the computer's screen and closes after; «оставь»
/// keeps it; several at once; the agent plays, pauses and stops a video by its address.
#[tokio::test]
async fn photos_and_videos_open_windows_on_the_screen_temporary_unless_kept() {
    let home = home(false).await;
    phrase(&home, "openFolder", json!({ "name": "фото" })).await;
    said(&home, "открой отпуск").await;
    assert_eq!(speech(&said(&home, "номер один").await), "Показываю «1».");
    assert_eq!(titles(&home), [("1".to_string(), false)]);
    said(&home, "следующая").await;
    assert_eq!(titles(&home), [("2".to_string(), false)], "the next photo takes the temporary window's place");
    assert_eq!(speech(&said(&home, "оставь").await), "Оставила на экране.");
    said(&home, "следующая").await;
    assert_eq!(titles(&home), [("2".to_string(), true), ("3".to_string(), false)]);
    assert_eq!(said(&home, "закрой").await, json!("Хорошо."));
    assert_eq!(titles(&home), [("2".to_string(), true)], "«закрой» closes the temporary windows, the kept one stays");

    // The agent: two photos and a video at once, beside the kept one.
    let shown = home
        .plugin
        .call(
            "toolShow",
            json!({ "addresses": ["files/home/Фото/Отпуск 2025/1.png", "files/home/Фото/Отпуск 2025/3.png", "files/home/Видео/Море.mp4"], "add": true }),
            CallKind::Tool,
        )
        .await
        .unwrap();
    assert_eq!(shown["opened"].as_array().unwrap().len(), 3, "{shown}");
    assert_eq!(home.screen.windows().len(), 4);
    let video = "files/home/Видео/Море.mp4";
    let pause = json!({ "address": video, "action": "pause" });
    home.plugin.call("toolPlay", pause, CallKind::Tool).await.unwrap();
    assert!(home.screen.windows().iter().find(|window| window.title == "Море").unwrap().paused);
    home.plugin.call("toolPlay", json!({ "address": video, "action": "stop" }), CallKind::Tool).await.unwrap();
    assert!(home.screen.windows().iter().all(|window| window.title != "Море"));
    let played = home.plugin.call("toolPlay", json!({ "address": video }), CallKind::Tool).await.unwrap();
    assert_eq!(played["playing"][0]["kind"], "video", "{played}");
    assert_eq!(titles(&home), [("2".to_string(), true), ("Море".to_string(), false)]);

    // No screen (the gateway on the station): the photo is on the desk's card only, the voice goes on.
    home.screen.set_available(false);
    phrase(&home, "openFolder", json!({ "name": "видео" })).await;
    let video = said(&home, "море").await;
    assert!(speech(&video).contains("на компьютере с приложением DuoWave"), "{video}");
}

/// Live 14.11: «все три открой» was «Хорошо.» (the word «все» ended the conversation); «открой скриншот» did not find
/// «Screenshot…»; a sound asked of the station through the agent went to a window on the computer, not to the station.
#[tokio::test]
async fn all_at_once_names_across_alphabets_and_a_sound_on_the_station() {
    let home = home(false).await;
    phrase(&home, "openFolder", json!({ "name": "фото" })).await;
    said(&home, "открой отпуск").await;
    assert_eq!(speech(&said(&home, "все три открой").await), "Открыла 3 окна.");
    assert_eq!(home.screen.windows().len(), 3);
    assert_eq!(said(&home, "всё").await, json!("Хорошо."));
    assert!(home.screen.windows().is_empty(), "«всё» alone ends it, the temporary windows close");

    phrase(&home, "openFolder", json!({ "name": "картинки" })).await;
    assert_eq!(speech(&said(&home, "открой скриншот").await), "Показываю «Screenshot 2026-11-13 at 16.01.11».");

    let sound = "files/home/Документы/Голос.wav";
    let station = json!({ "station": true });
    let input = json!({ "address": sound, "request": station });
    let played = home.plugin.call("toolOpen", input, CallKind::Tool).await.unwrap();
    assert_eq!(played["play"][0]["title"], "Голос", "{played}");
    assert_eq!(speech(&played), "Включаю «Голос».");
    let input = json!({ "action": "pause", "request": station });
    let paused = home.plugin.call("toolPlay", input, CallKind::Tool).await.unwrap();
    assert_eq!(paused, json!({ "player": "pause" }), "nothing plays in a window: the station's player");
    let input = json!({ "address": sound, "request": { "station": false } });
    let panel = home.plugin.call("toolOpen", input, CallKind::Tool).await.unwrap();
    assert_eq!(panel["shown"], "в окне на экране компьютера", "{panel}");
    assert_eq!(home.screen.windows()[0].title, "Голос");
}

#[tokio::test]
async fn photos_reading_finding_and_writing_by_voice() {
    let home = home(false).await;
    let photos = phrase(&home, "photos", json!({ "name": "отпуска" })).await;
    assert_eq!(speech(&photos), "В «Отпуск 2025» 3 фото. Показываю первое.");
    let read = phrase(&home, "find", json!({ "query": "борща" })).await;
    assert!(speech(&read).contains("Свёкла, капуста, картошка."), "{read}");
    let shown = card(&home).await;
    assert_eq!(shown["rows"][0]["text"], "Свёкла, капуста, картошка.");
    let written = phrase(&home, "write", json!({ "rest": "рецепты блины два яйца стакан муки" })).await;
    assert_eq!(speech(&written), "Записала «Блины» в «Рецепты».");
    assert_eq!(std::fs::read_to_string(home.root.join("Рецепты/Блины.txt")).unwrap(), "Блины два яйца стакан муки\n");
    let again = phrase(&home, "write", json!({ "rest": "рецепты блины три яйца" })).await;
    assert_eq!(speech(&again), "Записала «Блины 2» в «Рецепты».");
    let list = phrase(&home, "folders", json!({})).await;
    assert!(speech(&list).starts_with("Папки: "), "{list}");
    assert!(speech(&list).contains("Рецепты"), "a folder a person made in Finder is there: {list}");
}

#[tokio::test]
async fn yandexs_voice_never_hears_a_name_without_the_tick() {
    let home = home(true).await;
    let listed = phrase(&home, "openFolder", json!({ "name": "рецепты" })).await;
    assert_eq!(speech(&listed), "В «Рецепты» 1 папка и 1 файл. Они на экране.");
    let read = said(&home, "прочитай щи").await;
    assert_eq!(speech(&read), "Текст на экране.");
    assert_eq!(card(&home).await["rows"][0]["text"], "Капуста.");
    home.folders
        .change(&Address::parse("files/home/Рецепты").unwrap(), &Change { speak: Some(true), ..Change::default() })
        .unwrap();
    let ticked = phrase(&home, "openFolder", json!({ "name": "рецепты" })).await;
    assert_eq!(speech(&ticked), "В «Рецепты» 1 папка и 1 файл: Супы, Щи. Что открыть?");
}

#[tokio::test]
async fn a_drive_offered_is_opened_by_yes() {
    let home = home(false).await;
    let usb = home._dir.path().join("Volumes/KINGSTON");
    std::fs::create_dir_all(usb.join("DCIM")).unwrap();
    home.drives.step(vec![Drive { mark: "KINGSTON:64".into(), label: "KINGSTON".into(), path: usb.clone() }]);
    let offer = home.drives.offers()[0].clone();
    let asked = phrase(&home, "driveFound", json!({ "name": offer.name, "offer": offer.id })).await;
    assert_eq!(speech(&asked), "Подключена флешка kingston. Посмотреть, что на ней?");
    assert_eq!(asked["continue"]["handler"], "driveAnswer");
    let input = json!({ "text": "да", "request": REQUEST() });
    let opened = home.plugin.call("driveAnswer", input, CallKind::Voice).await.unwrap();
    assert_eq!(speech(&opened), "Открыла. В «Флешка KINGSTON» 1 папка: DCIM. Что открыть?");
    assert!(home.drives.offers().is_empty());
    assert_eq!(home.folders.attached().len(), 1);
    let _ = &home.host;
}
