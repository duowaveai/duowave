//! What works where — one registry of the gateway's features (docs/plans/station-placement.md, section 1). A feature
//! is described once: the ways it can run, in order of preference, and what each way needs. The gateway, the panel
//! of the computer and the panel of the station all read it from here: whether a feature is on now, where it runs
//! (here, at a helper, in the cloud, at Yandex's) and why not. A new feature is a row in `FEATURES` (and one
//! `available('…')` in the panel where it is to be greyed out).
//!
//! The heavy parts — hearing, voice, a model, the apps — are the helpers' (src/helpers, docs/plans/home-and-helpers.md):
//! a way that needs one asks whether a helper gives it now, the same question the choice of road asks
//! (`Helpers::pick`). On a computer its own heavy part is the helper, and the way runs here.
//!
//! Not to be confused with the routes' capabilities (`core/capabilities.rs`): those are what the station can do for
//! the household; these are what this gateway can do where it runs.

use serde::{Deserialize, Serialize};

use crate::core::helpers::contract::{Abilities, Ability};
use crate::settings::definitions::PanelMode;

/// Where the gateway runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Placement {
    /// On a computer: the station talks to it over the home network (as before).
    Pc,
    /// On the station itself: on its own, trimmed.
    Station,
    /// On the station, with a computer at home as its server for the heavy parts while it is up.
    StationServer,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Pc, Placement::Station, Placement::StationServer];

    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Pc => "pc",
            Placement::Station => "station",
            Placement::StationServer => "stationServer",
        }
    }

    pub fn parse(value: &str) -> Option<Placement> {
        Placement::ALL.into_iter().find(|placement| placement.as_str() == value)
    }

    pub fn on_station(self) -> bool {
        self != Placement::Pc
    }

    /// The three texts of a placement in the language of now (`title` and the rest are the source texts).
    pub fn title_text(self) -> String {
        super::tx(self.title())
    }

    pub fn summary_text(self) -> String {
        super::tx(self.summary())
    }

    pub fn risk_text(self) -> String {
        super::tx(self.risk())
    }

    pub fn title(self) -> &'static str {
        match self {
            Placement::Pc => "На компьютере",
            Placement::Station => "На колонке",
            Placement::StationServer => "На колонке + домашний сервер",
        }
    }

    /// For the choice of placement: what it gives, and the risk taken.
    pub fn summary(self) -> &'static str {
        match self {
            Placement::Pc => "Колонка говорит с хабом на компьютере. Всё своё: распознавание, голос, модели, звонки.",
            Placement::Station => {
                "Хаб на самой колонке: работает без компьютера, с облачными подключениями или Яндексом. Без своих моделей и звонков."
            }
            Placement::StationServer => {
                "Хаб на колонке, а тяжёлое — распознавание, свой голос, модели — он берёт у компьютера, пока тот на связи. Компьютер спит — работает через облако или Яндекс."
            }
        }
    }

    pub fn risk(self) -> &'static str {
        match self {
            Placement::Pc => "Компьютер спит или выключен — колонка молчит.",
            Placement::Station => {
                "Мало памяти и ОЗУ на колонке; служба перезапускает DuoWave, страховка возвращает Яндекс."
            }
            Placement::StationServer => {
                "Без компьютера — урезанно; «Ваш ИИ» — пока Турбо или облако дают слух и голос."
            }
        }
    }
}

/// Where a feature runs now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Runs {
    /// In this gateway: on the computer — its own heavy part included —, or on the station.
    Here,
    /// At a helper the gateway leans on (src/helpers): the computer at home, one's own server, the DuoWave cloud. Said
    /// `"homeServer"` for one more version, as the panels of 0.11 read it; `"helper"` a version later.
    #[serde(rename = "homeServer")]
    Helper,
    Cloud,
    Yandex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    Modes,
    Voice,
    Brain,
    Skills,
    House,
    System,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Modes => "Режимы",
            Group::Voice => "Голос",
            Group::Brain => "Мышление",
            Group::Skills => "Навыки",
            Group::House => "Дом",
            Group::System => "Система",
        }
    }
}

/// What a way needs. `PcBuild` and `At` say whether the way exists in this placement at all, `Helper` and `HelperUp`
/// whether a helper is set up for it (`Context::exists`); then each need says whether the way works right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// The computer's build: local model servers, the voice lab, calls, SSH, MCP programs are compiled in.
    PcBuild,
    /// One of these placements.
    At(&'static [Placement]),
    /// A helper gives this ability now — the one `Helpers::pick` takes; on a computer, its own heavy part. The way is
    /// there once some helper is set up; it works while one gives the ability.
    Helper(Ability),
    /// A helper of this gateway is on the line, whatever it gives: «Свой» on the station leans on it, and the
    /// transcoding the computer does for it.
    HelperUp,
    /// A recognizer in the cloud is set up, and the regulator lets the clouds out.
    CloudAsr,
    /// A voice in the cloud is set up, and the regulator lets the clouds out.
    CloudTts,
    /// The regulator «Что может уходить из дома» lets the clouds by keys out («Любые облака»).
    Clouds,
    /// Yandex may serve: the mode is not «Свой».
    Yandex,
    /// This much of the plugins' memory is free here (src/plugins/budget.rs): on the station it is short.
    Memory(u64),
    /// Streams can be transcoded here: a transcoder (ffmpeg) on a computer, or on the station by the owner's flag
    /// (MEDIA_TRANSCODE_ON_STATION) — it loads the station's weak processor.
    Transcode,
    /// «Свой» on the station hears and speaks: a helper gives it hearing and voice, or a cloud the regulator lets out
    /// stands in for the part none gives — without Yandex nothing else would.
    OwnParts,
    /// This gateway hears MIDI keyboards: a computer's build with `midi`, and not `MIDI=0` (src/midi).
    Midi,
    /// A Турбо at home has a MIDI keyboard plugged in now (docs/plans/turbo-devices.md): the centre relays its notes.
    TurboMidi,
}

pub struct Way {
    pub runs: Runs,
    pub needs: &'static [Need],
}

/// How a feature fares in a placement, for the choice of placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Yes,
    Partial,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Cell {
    pub level: Level,
    #[serde(serialize_with = "crate::core::translated")]
    pub note: &'static str,
}

const fn yes(note: &'static str) -> Cell {
    Cell { level: Level::Yes, note }
}

const fn partial(note: &'static str) -> Cell {
    Cell { level: Level::Partial, note }
}

const fn no(note: &'static str) -> Cell {
    Cell { level: Level::No, note }
}

pub struct Feature {
    /// `mode.own`, `asr.own`, …: the panel asks by it.
    pub id: &'static str,
    pub title: &'static str,
    pub group: Group,
    /// The ways it can run, in order of preference: the first that works is where it runs now.
    pub ways: &'static [Way],
    /// Why it is not in a placement that has no way for it at all.
    pub absent: &'static str,
    /// For the choice of placement, in the order of `Placement::ALL`: what it is there.
    pub table: [Cell; 3],
}

const ON_STATION_WITH_SERVER: &[Placement] = &[Placement::StationServer];
const ON_STATION: &[Placement] = &[Placement::Station, Placement::StationServer];

/// Why «Свой» is not on the station: the switch refuses it with this, the panel says it. Since 16.11 the station
/// takes «Своё» with a cloud hearing and voice (docs/plans/speaker-alone.md, stage 1): this is said when neither the
/// computer nor the cloud gives them.
pub const OWN_ON_STATION: &str = "«Ваш ИИ» на колонке без Яндекса: нужны облачный слух и облачный голос \
(«Голос» и «Слух» в настройках колонки, хватит ключа OpenAI) или Турбо.";

pub const FEATURES: &[Feature] = &[
    Feature {
        id: "mode.relay",
        title: "Режим «Алиса»",
        group: Group::Modes,
        ways: &[Way { runs: Runs::Yandex, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "mode.gateway",
        title: "Режим «Алиса + ваш ИИ»",
        group: Group::Modes,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "mode.own",
        title: "Режим «Ваш ИИ»",
        group: Group::Modes,
        // On the station «Свой» leans on a helper on the line that gives it hearing and voice (or a cloud stands in) —
        // the computer at home or another helper: the placement's «колонка + сервер» stays the computer's own.
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::Helper, needs: &[Need::At(ON_STATION), Need::HelperUp, Need::OwnParts] },
            // The station by itself: it hears and speaks through the cloud (src/speech/cloud_speech.rs).
            Way { runs: Runs::Cloud, needs: &[Need::At(ON_STATION), Need::CloudAsr, Need::CloudTts] },
        ],
        absent: OWN_ON_STATION,
        table: [yes("да"), partial("с облачным слухом и голосом"), partial("компьютер → облако")],
    },
    Feature {
        id: "asr.own",
        title: "Своё распознавание",
        group: Group::Voice,
        ways: &[
            Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Hearing)] },
            Way { runs: Runs::Cloud, needs: &[Need::CloudAsr] },
            Way { runs: Runs::Yandex, needs: &[Need::Yandex] },
        ],
        absent: "",
        table: [
            yes("на компьютере"),
            partial("облако (подключение) или Яндекс"),
            partial("компьютер → облако → Яндекс"),
        ],
    },
    Feature {
        id: "voice.own",
        title: "Свой голос",
        group: Group::Voice,
        ways: &[
            Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Voice)] },
            Way { runs: Runs::Cloud, needs: &[Need::CloudTts] },
            Way { runs: Runs::Yandex, needs: &[Need::Yandex] },
        ],
        absent: "",
        table: [yes("на компьютере"), partial("облако или голос Алисы"), partial("компьютер → облако → голос Алисы")],
    },
    Feature {
        id: "voice.cloned",
        title: "Голоса из лаборатории",
        group: Group::Voice,
        ways: &[Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Voice)] }],
        absent: "Голоса из лаборатории звучат только с компьютера: их модели там.",
        table: [yes("да"), no("нет"), partial("через компьютер")],
    },
    Feature {
        id: "brain.cloud",
        title: "Мышление: облачные модели",
        group: Group::Brain,
        ways: &[Way { runs: Runs::Cloud, needs: &[Need::Clouds] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "brain.local",
        title: "Мышление: модель на компьютере",
        group: Group::Brain,
        ways: &[Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Model)] }],
        absent: "Своя модель работает на компьютере: на колонке ей не хватит ни ОЗУ, ни процессора.",
        table: [yes("да"), no("нет"), partial("через компьютер")],
    },
    Feature {
        id: "skills",
        title: "Навыки, маршруты, напоминания",
        group: Group::Skills,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "mcp.http",
        title: "MCP по сети",
        group: Group::Skills,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "mcp.programs",
        title: "MCP-программы (npx, uvx)",
        group: Group::Skills,
        ways: &[Way { runs: Runs::Here, needs: &[Need::PcBuild] }],
        absent: "MCP-программы запускаются только на компьютере: на колонке нет ни Node, ни Python.",
        table: [yes("да"), no("нет"), no("нет (позже — через компьютер)")],
    },
    Feature {
        id: "connectors",
        title: "Home Assistant, новости, погода",
        group: Group::House,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    // The bot is the calls module's, the computer's build only: on the station «передай …» goes through a helper's
    // (src/home_server/messenger.rs).
    Feature {
        id: "messages",
        title: "Сообщения в Telegram: «передай …» и входящие",
        group: Group::House,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Messages)] },
        ],
        absent: "Сообщения в Telegram уходят через бота компьютера: в сборке для колонки его нет.",
        table: [yes("да"), no("нет"), partial("через компьютер")],
    },
    Feature {
        id: "calls",
        title: "Звонки: страница, SIP, LiveKit",
        group: Group::House,
        ways: &[Way { runs: Runs::Here, needs: &[Need::PcBuild] }],
        absent: "Звонки — только в DuoWave на компьютере: страница звонков, SIP и LiveKit там.",
        table: [yes("да"), no("нет"), no("нет (позже)")],
    },
    Feature {
        id: "stations.connect",
        title: "Поиск и подключение колонок",
        group: Group::System,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::Helper, needs: &[Need::At(ON_STATION_WITH_SERVER)] },
        ],
        absent: "Колонки ищет и подключает DuoWave на компьютере: в сборке для колонки нет SSH.",
        table: [yes("да"), no("нет"), partial("с компьютера")],
    },
    Feature {
        id: "voice.lab",
        title: "Голосовая лаборатория",
        group: Group::Voice,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::Helper, needs: &[Need::At(ON_STATION_WITH_SERVER)] },
        ],
        absent: "Голосовая лаборатория — только на компьютере.",
        table: [yes("да"), no("нет"), partial("на компьютере")],
    },
    Feature {
        id: "characters",
        title: "Персонажи",
        group: Group::Brain,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    // The characters' memory (src/memory): the household graph, «запомни», the wake-up — the same on a computer and a
    // station; search by meaning and reflection come with their stages (docs/plans/memory.md).
    Feature {
        id: "memory",
        title: "Память персонажей",
        group: Group::Brain,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("факты, связи, поиск по словам"), yes("да")],
    },
    Feature {
        id: "characters.voice",
        title: "Голоса персонажей",
        group: Group::Voice,
        ways: &[Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Voice)] }],
        absent: "Голоса персонажей звучат с компьютера: их модели там. На колонке говорит запасной голос.",
        table: [yes("да"), no("нет: запасной голос"), partial("через компьютер")],
    },
    Feature {
        id: "plugins",
        title: "Приложения",
        group: Group::Skills,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), partial("в пределах ОЗУ колонки"), partial("в пределах ОЗУ колонки")],
    },
    Feature {
        id: "plugins.apps",
        title: "Приложения (Spotify и другие)",
        group: Group::Skills,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::Memory(APP_MEMORY_MB)] },
            Way { runs: Runs::Helper, needs: &[Need::Helper(Ability::Apps)] },
        ],
        absent: "",
        table: [
            yes("да"),
            partial("если хватит ОЗУ колонки"),
            partial("на компьютере; без него — если хватит ОЗУ"),
        ],
    },
    Feature {
        id: "media.transcode",
        title: "Перекодирование потоков (AAC → MP3)",
        group: Group::Skills,
        // No helper's contract has transcoding: the station's streams go to the computer that helps it, as before.
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::Transcode] },
            Way { runs: Runs::Helper, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HelperUp] },
        ],
        absent: "На колонке перекодирование выключено: большая нагрузка на её процессор (MEDIA_TRANSCODE_ON_STATION).",
        table: [yes("да"), no("нет — большая нагрузка"), partial("на компьютере")],
    },
    Feature {
        id: "plugins.midi",
        title: "MIDI-клавиатура для приложений",
        group: Group::Skills,
        // At the centre's own keyboard, or at a Турбо's at home: the centre relays its notes (docs/plans/turbo-devices.md).
        ways: &[Way { runs: Runs::Here, needs: &[Need::Midi] }, Way { runs: Runs::Helper, needs: &[Need::TurboMidi] }],
        absent: "Клавиатуру подключают к компьютеру с DuoWave, у колонки её нет. Подключите компьютер к дому как Турбо и подключите клавиатуру к нему.",
        table: [yes("да, клавиатура — к компьютеру"), no("нет: клавиатуру подключают к компьютеру"), partial("клавиатуру подключают к Турбо")],
    },
    Feature {
        id: "offline",
        title: "Работает, когда компьютер спит",
        group: Group::System,
        ways: &[Way { runs: Runs::Here, needs: &[Need::At(ON_STATION)] }],
        absent: "Хаб на компьютере: компьютер спит — колонка молчит.",
        table: [no("нет"), yes("да"), partial("да, урезанно")],
    },
];

/// What an app takes of the station's memory, at least (librespot: 20–30 MB).
pub const APP_MEMORY_MB: u64 = 30;

pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|feature| feature.id == id)
}

/// The helpers together as the station's gateway sees them — the old view of its home server, which panels and
/// computers of 0.11 read (`homeServer` in `/api/v1/availability` and `/health`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HomeServer {
    /// None set up.
    None,
    Up,
    Down,
}

/// The helpers as the registry reads them (src/helpers/registry.rs). `Copy`, so the context stays `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperContext {
    /// A helper of this gateway is on the line, whatever it gives; on a computer, its own heavy part.
    pub online: bool,
    /// What a helper the gateway may take from is set to give: declared by it (before its first answer — all of
    /// contract 1), not switched off for it, its road let out by the regulator.
    pub configured: Abilities,
    /// Of those, what a helper gives now: exactly what `Helpers::pick` finds.
    pub up: Abilities,
    /// Of those not given, what the helper that would give it declares while on the line — its part still loading,
    /// its bot's contacts not read yet, no bot at all (`no_bot`): «не готов», not «не на связи».
    pub waiting: Abilities,
    /// Of those waiting, the messages: the helper that would pass them says its Telegram bot is not set up — its
    /// contacts answer so, or its contract (of version 1) declares them not ready —: «не настроен бот», not «не готов».
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_bot: bool,
    /// What this computer's own heavy part gives — the built-in helper —, said «на этом компьютере». Empty on a
    /// station: it takes nothing from itself.
    pub builtin: Abilities,
    /// Helpers are set up, and the regulator keeps every one of them out: none to lean on for now, not none at all —
    /// the helpers' ways stay and say so, and a stored «Свой» is kept for the gateway to leave and come back to.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shut: bool,
}

impl HelperContext {
    /// Some helper the gateway may take from is set up.
    pub const fn set_up(self) -> bool {
        !self.configured.is_empty()
    }

    /// The helpers' ways are there at all: some helper is set up, let out by the regulator now or not.
    pub const fn there(self) -> bool {
        self.set_up() || self.shut
    }

    /// With what this computer's own heavy part gives: set up, given now, and on the line — never kept out.
    pub fn with_builtin(self, abilities: Abilities) -> HelperContext {
        HelperContext {
            online: self.online || !abilities.is_empty(),
            configured: self.configured.union(abilities),
            up: self.up.union(abilities),
            waiting: self.waiting.iter().filter(|ability| !abilities.has(*ability)).collect(),
            no_bot: self.no_bot && !abilities.has(Ability::Messages),
            builtin: self.builtin.union(abilities),
            shut: self.shut && abilities.is_empty(),
        }
    }

    /// Every helper set up — the ones the regulator keeps out too — counted as on the line and giving what it is set
    /// to: whether a way is there at all, as opposed to working right now.
    pub const fn all_up(self) -> HelperContext {
        HelperContext {
            online: self.there(),
            configured: self.configured,
            up: self.configured,
            waiting: Abilities::NONE,
            no_bot: false,
            builtin: self.builtin,
            shut: self.shut,
        }
    }
}

/// Why a helper's way is not taken: the helper is away (the same words as «Свой» leaving on the station).
pub const HELPER_AWAY: &str = crate::core::helpers::AWAY;
/// Why no helper's way is taken: the regulator keeps every helper out (the same words as «Свой» leaving on the
/// station).
pub const HELPERS_SHUT: &str = crate::core::helpers::SHUT;
/// Why a helper on the line does not give its messages: it says its Telegram bot is not set up.
pub const HELPER_NO_BOT: &str = "у Турбо не настроен бот Telegram";
/// Why a helper on the line does not give an ability yet: its model, its apps, its bot's contacts not read yet.
pub const HELPER_NOT_READY: &str = "Турбо ещё не готово";
/// Why a cloud's way is not taken: «Только дома» or «Дом и моё облако» keep the clouds by keys out. «Мозг» says it
/// too, of a cloud model it may not ask now (`BrainNow.reason`, dialog/brain/brain_service.rs).
pub const CLOUDS_SHUT: &str = "облака закрыты регулятором";

/// Why the helper on the line does not give an ability it declares (the same words as «Свой» on the station); its
/// bot not set up is said apart (`HELPER_NO_BOT`).
pub fn helper_not_ready(ability: Ability) -> &'static str {
    match ability {
        Ability::Hearing => crate::core::helpers::HEARING_NOT_READY,
        Ability::Voice => crate::core::helpers::VOICE_NOT_READY,
        _ => HELPER_NOT_READY,
    }
}

/// Why no helper gives an ability: none the gateway may take from declares it, or each is kept from it.
pub fn no_helper_with(ability: Ability) -> &'static str {
    match ability {
        Ability::Hearing => crate::core::helpers::NO_HEARING,
        Ability::Voice => crate::core::helpers::NO_VOICE,
        Ability::Model => "нет Турбо с моделью",
        Ability::Messages => "нет Турбо с сообщениями",
        Ability::Apps => "нет Турбо с приложениями",
        Ability::History => "нет Турбо с историей приложений",
        Ability::Catalog => "нет Турбо с каталогом",
        Ability::Devices => "нет Турбо с устройствами",
        Ability::Files => "нет Турбо с папками",
        // The later blocks' abilities: no feature needs them yet.
        Ability::Tasks | Ability::Vault | Ability::Vectors | Ability::Panel => "нет Турбо с этим умением",
    }
}

/// What the evaluation reads: the build, the placement, the helpers, the clouds and the regulator, the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub pc_build: bool,
    pub placement: Placement,
    /// The helpers as this gateway takes from them now; a computer's own heavy part among them.
    pub helpers: HelperContext,
    pub cloud_asr: bool,
    pub cloud_tts: bool,
    /// The regulator «Что может уходить из дома» lets the clouds by keys out («Любые облака»).
    pub clouds: bool,
    /// Not «Свой».
    pub yandex: bool,
    /// The plugins' free memory here, when it is counted (on the station); none: not short.
    pub memory_free_mb: Option<u64>,
    /// A transcoder is here (ffmpeg found).
    pub transcoder: bool,
    /// MEDIA_TRANSCODE_ON_STATION: the owner lets the station transcode.
    pub transcode_on_station: bool,
    /// What «Свой» would lack on the station as the helpers give now: hearing or a voice that no helper gives and no
    /// cloud stands in for (src/helpers, `svoi_lacks`) — said as the reason; none: nothing.
    pub own_lacks: Option<&'static str>,
    /// This gateway hears MIDI keyboards (src/midi).
    pub midi: bool,
    /// A Турбо at home has a MIDI keyboard plugged in now (src/home/turbo_devices.rs).
    pub turbo_midi: bool,
}

impl Context {
    /// The best a placement can do: for the comparison of placements. A computer gives everything itself; the
    /// station alone has no helper; the station with its computer has it on the line, giving all it may.
    pub fn best(placement: Placement) -> Context {
        let helpers = match placement {
            Placement::Pc => HelperContext::default().with_builtin(Abilities::V1),
            Placement::Station => HelperContext::default(),
            Placement::StationServer => {
                HelperContext { online: true, configured: Abilities::V1, up: Abilities::V1, ..HelperContext::default() }
            }
        };
        Context {
            pc_build: placement == Placement::Pc,
            placement,
            helpers,
            cloud_asr: true,
            cloud_tts: true,
            clouds: true,
            yandex: true,
            memory_free_mb: None,
            transcoder: true,
            transcode_on_station: false,
            own_lacks: None,
            midi: placement == Placement::Pc,
            turbo_midi: placement == Placement::StationServer,
        }
    }

    /// Whether a way with this need is there at all, as opposed to working right now: the build, the placement, a
    /// transcoder, MIDI — and for a helper's way, some helper set up, the regulator's kept out for now included.
    fn exists(&self, need: Need) -> bool {
        match need {
            Need::PcBuild | Need::At(_) | Need::Transcode | Need::Midi => self.meets(need),
            Need::Helper(_) | Need::HelperUp | Need::TurboMidi => self.helpers.there(),
            _ => true,
        }
    }

    fn meets(&self, need: Need) -> bool {
        match need {
            Need::PcBuild => self.pc_build,
            Need::At(placements) => placements.contains(&self.placement),
            Need::Helper(ability) => self.helpers.up.has(ability),
            Need::HelperUp => self.helpers.online,
            Need::CloudAsr => self.cloud_asr && self.clouds,
            Need::CloudTts => self.cloud_tts && self.clouds,
            Need::Clouds => self.clouds,
            Need::Yandex => self.yandex,
            Need::Memory(mb) => self.memory_free_mb.is_none_or(|free| free >= mb),
            Need::Transcode => self.transcoder && (self.placement == Placement::Pc || self.transcode_on_station),
            Need::OwnParts => self.own_lacks.is_none(),
            Need::Midi => self.midi,
            Need::TurboMidi => self.turbo_midi,
        }
    }

    /// Why a need is not met, for people.
    fn why(&self, need: Need) -> &'static str {
        match need {
            Need::PcBuild | Need::At(_) => "не в этом размещении",
            Need::Helper(_) | Need::HelperUp if self.helpers.shut => HELPERS_SHUT,
            Need::Helper(ability) if !self.helpers.configured.has(ability) => no_helper_with(ability),
            Need::Helper(ability) if self.helpers.waiting.has(ability) => match ability {
                Ability::Messages if self.helpers.no_bot => HELPER_NO_BOT,
                _ => helper_not_ready(ability),
            },
            Need::Helper(_) | Need::HelperUp => HELPER_AWAY,
            Need::CloudAsr if self.clouds => "нет облачного распознавания",
            Need::CloudTts if self.clouds => "нет облачного голоса",
            Need::CloudAsr | Need::CloudTts | Need::Clouds => CLOUDS_SHUT,
            Need::Yandex => "в режиме «Ваш ИИ» Яндекс выключен",
            Need::Memory(_) => "на колонке не хватит ОЗУ",
            Need::Transcode => "нет перекодировщика",
            Need::OwnParts => self.own_lacks.unwrap_or("компьютер не даёт колонке слух или голос"),
            Need::Midi => "в этой сборке MIDI нет или он выключен (MIDI=0)",
            Need::TurboMidi => "клавиатура не подключена ни к одному Турбо дома",
        }
    }

    /// Where a way runs: a helper's way whose every ability this computer's own heavy part gives runs here — that
    /// helper is this gateway. A helper's way that names no ability (the station leaning on its helper) stays a
    /// helper's.
    fn runs_of(&self, way: &Way) -> Runs {
        let mut abilities = way
            .needs
            .iter()
            .filter_map(|need| match need {
                Need::Helper(ability) => Some(*ability),
                _ => None,
            })
            .peekable();
        let builtin = abilities.peek().is_some() && abilities.all(|ability| self.helpers.builtin.has(ability));
        if way.runs == Runs::Helper && builtin {
            Runs::Here
        } else {
            way.runs
        }
    }

    /// Where a way runs, for people.
    fn place(&self, runs: Runs) -> &'static str {
        match runs {
            Runs::Here if self.placement == Placement::Pc => "на этом компьютере",
            Runs::Here => "на колонке",
            Runs::Helper => "у Турбо",
            Runs::Cloud => "через облако",
            Runs::Yandex => "у Яндекса",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// Its first way works.
    Ready,
    /// A later way works: on the way down («помощник не на связи — через облако»).
    Fallback,
    Off,
}

/// A feature now, as the gateway and the panels read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureState {
    pub id: &'static str,
    #[serde(serialize_with = "crate::core::translated")]
    pub title: &'static str,
    pub group: Group,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<Runs>,
    pub reason: String,
}

impl FeatureState {
    pub fn on(&self) -> bool {
        self.state != State::Off
    }
}

/// A feature in a context: the first way here that works, and why the ones before it do not.
pub fn evaluate(feature: &'static Feature, context: &Context) -> FeatureState {
    let state = |state: State, runs: Option<Runs>, reason: String| FeatureState {
        id: feature.id,
        title: feature.title,
        group: feature.group,
        state,
        runs,
        reason,
    };
    // The ways this placement has at all; the first of them is the one meant.
    let here: Vec<&Way> =
        feature.ways.iter().filter(|way| way.needs.iter().all(|need| context.exists(*need))).collect();
    let Some(meant) = here.first() else {
        return state(State::Off, None, super::tx(feature.absent));
    };
    let unmet = |way: &Way| way.needs.iter().copied().find(|need| !context.meets(*need));
    match here.iter().enumerate().find(|(_, way)| unmet(way).is_none()) {
        Some((0, way)) => {
            let runs = context.runs_of(way);
            state(State::Ready, Some(runs), super::tx(context.place(runs)))
        }
        Some((_, way)) => {
            let runs = context.runs_of(way);
            let why = unmet(meant).map(|need| super::tx(context.why(need))).unwrap_or_default();
            state(State::Fallback, Some(runs), format!("{why} — {}", super::tx(context.place(runs))))
        }
        None => {
            let why = unmet(meant).map(|need| super::tx(context.why(need))).unwrap_or_default();
            state(State::Off, None, capitalized(&why))
        }
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Every feature in a context.
pub fn evaluate_all(context: &Context) -> Vec<FeatureState> {
    FEATURES.iter().map(|feature| evaluate(feature, context)).collect()
}

/// The feature's id for a mode, for the mode switch.
pub fn mode_feature(mode: PanelMode) -> &'static str {
    match mode {
        PanelMode::Relay => "mode.relay",
        PanelMode::Gateway => "mode.gateway",
        PanelMode::Own => "mode.own",
    }
}

/// A row of the comparison of placements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub id: &'static str,
    #[serde(serialize_with = "crate::core::translated")]
    pub title: &'static str,
    pub group: Group,
    pub pc: Cell,
    pub station: Cell,
    pub station_server: Cell,
}

pub fn table() -> Vec<TableRow> {
    FEATURES
        .iter()
        .map(|feature| TableRow {
            id: feature.id,
            title: feature.title,
            group: feature.group,
            pc: feature.table[0],
            station: feature.table[1],
            station_server: feature.table[2],
        })
        .collect()
}

/// The panel's list of ids (panel/src/availability-ids.ts), as this registry writes it: a test keeps the file the
/// same, so the panel never asks for a feature the gateway does not know.
pub fn panel_ids() -> String {
    let mut text = String::from(
        "// The gateway's features (gateway/src/core/availability.rs), written by its tests: do not edit by hand.\n\
         // DUOWAVE_WRITE_PANEL_IDS=1 cargo test -p duowave availability writes it again after a change.\n\n\
         export const FEATURE_IDS = [\n",
    );
    for feature in FEATURES {
        text.push_str(&format!("    '{}',\n", feature.id));
    }
    text.push_str("] as const;\n\nexport type FeatureId = (typeof FEATURE_IDS)[number];\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(placement: Placement) -> Context {
        Context::best(placement)
    }

    #[test]
    fn every_feature_says_what_it_is_in_every_placement_and_why_it_is_absent() {
        let mut ids = std::collections::BTreeSet::new();
        for feature in FEATURES {
            assert!(ids.insert(feature.id), "{} twice", feature.id);
            assert!(!feature.title.is_empty() && !feature.ways.is_empty(), "{}", feature.id);
            for (placement, cell) in Placement::ALL.iter().zip(feature.table) {
                assert!(!cell.note.is_empty(), "{} has no note for {}", feature.id, placement.as_str());
                // The table agrees with the ways: «нет» where no way works even at best, and the other way round.
                let best = evaluate(feature, &at(*placement));
                assert_eq!(
                    cell.level == Level::No,
                    !best.on(),
                    "{} in {}: the table says {:?}, the ways {:?}",
                    feature.id,
                    placement.as_str(),
                    cell.level,
                    best.state
                );
                if !best.on() {
                    assert!(!best.reason.is_empty(), "{} says nothing in {}", feature.id, placement.as_str());
                }
            }
        }
    }

    /// The station with its computer, the computer away: set up to give all it may, giving nothing now.
    fn away() -> Context {
        let server = at(Placement::StationServer);
        Context {
            helpers: HelperContext { online: false, up: Abilities::NONE, ..server.helpers },
            turbo_midi: false,
            ..server
        }
    }

    /// A helper on the line that gives what it is set to: these abilities.
    fn giving(abilities: &[Ability]) -> HelperContext {
        let abilities = Abilities::of(abilities);
        HelperContext { online: true, configured: abilities, up: abilities, ..HelperContext::default() }
    }

    /// One answer of the registry: the state, and where the feature runs as the wire says it.
    type Answer = (State, Option<&'static str>);
    const HERE: Answer = (State::Ready, Some("here"));
    const HELPER: Answer = (State::Ready, Some("homeServer"));
    const CLOUD: Answer = (State::Ready, Some("cloud"));
    const YANDEX: Answer = (State::Ready, Some("yandex"));
    const DOWN_TO_CLOUD: Answer = (State::Fallback, Some("cloud"));
    const OFF: Answer = (State::Off, None);

    /// The registry's answers before the helpers (0.11.1, worked out from its ways then): each feature on the
    /// computer, on the station alone, on the station with its computer, and with that computer away.
    const BEFORE: &[(&str, [Answer; 4])] = &[
        ("mode.relay", [YANDEX, YANDEX, YANDEX, YANDEX]),
        ("mode.gateway", [HERE, HERE, HERE, HERE]),
        // «Свой» on the station alone and with its computer away: through the cloud since speaker-alone stage 1.
        ("mode.own", [HERE, CLOUD, HELPER, DOWN_TO_CLOUD]),
        ("asr.own", [HERE, CLOUD, HELPER, DOWN_TO_CLOUD]),
        ("voice.own", [HERE, CLOUD, HELPER, DOWN_TO_CLOUD]),
        ("voice.cloned", [HERE, OFF, HELPER, OFF]),
        ("brain.cloud", [CLOUD, CLOUD, CLOUD, CLOUD]),
        ("brain.local", [HERE, OFF, HELPER, OFF]),
        ("skills", [HERE, HERE, HERE, HERE]),
        ("mcp.http", [HERE, HERE, HERE, HERE]),
        ("mcp.programs", [HERE, OFF, OFF, OFF]),
        ("connectors", [HERE, HERE, HERE, HERE]),
        ("messages", [HERE, OFF, HELPER, OFF]),
        ("calls", [HERE, OFF, OFF, OFF]),
        ("stations.connect", [HERE, OFF, HELPER, HELPER]),
        ("voice.lab", [HERE, OFF, HELPER, HELPER]),
        ("characters", [HERE, HERE, HERE, HERE]),
        ("memory", [HERE, HERE, HERE, HERE]),
        ("characters.voice", [HERE, OFF, HELPER, OFF]),
        ("plugins", [HERE, HERE, HERE, HERE]),
        ("plugins.apps", [HERE, HERE, HERE, HERE]),
        ("media.transcode", [HERE, OFF, HELPER, OFF]),
        // A Турбо's keyboard through the centre since docs/plans/turbo-devices.md.
        ("plugins.midi", [HERE, OFF, HELPER, OFF]),
        ("offline", [OFF, HERE, HERE, HERE]),
    ];

    /// «Готово, когда: таблица реестра для трёх размещений даёт прежние ответы»: with the helpers in place of the home
    /// server every feature keeps its state and where it runs in the three placements and with the computer away —
    /// the helpers' way said `homeServer` on the wire, as the panels of 0.11 read it.
    #[test]
    fn the_registry_gives_the_answers_it_gave_before_helpers() {
        let contexts = [
            ("pc", at(Placement::Pc)),
            ("station", at(Placement::Station)),
            ("stationServer", at(Placement::StationServer)),
            ("stationServer, the computer away", away()),
        ];
        let listed: Vec<&str> = BEFORE.iter().map(|(id, _)| *id).collect();
        let features: Vec<&str> = FEATURES.iter().map(|feature| feature.id).collect();
        assert_eq!(listed, features, "every feature has its old answers here");
        for (id, answers) in BEFORE {
            for ((name, context), answer) in contexts.iter().zip(answers) {
                let now = evaluate(feature(id).unwrap(), context);
                let runs = now.runs.map(|runs| serde_json::to_value(runs).unwrap());
                let runs = runs.as_ref().map(|runs| runs.as_str().unwrap());
                assert_eq!((now.state, runs), *answer, "{id} in {name}: {}", now.reason);
            }
        }
    }

    /// «… а причины — новыми словами»: a helper's way says «у помощника» on the line and «помощник не на связи» away,
    /// where it said the computer; this computer's own heavy part stays «на этом компьютере». No reason of the four
    /// contexts keeps the old words.
    #[test]
    fn the_reasons_say_helper_where_they_said_computer() {
        let reason = |id: &str, context: &Context| evaluate(feature(id).unwrap(), context).reason;
        let server = at(Placement::StationServer);
        let leaning = [
            "mode.own",
            "asr.own",
            "voice.own",
            "voice.cloned",
            "brain.local",
            "messages",
            "stations.connect",
            "voice.lab",
            "characters.voice",
            "media.transcode",
        ];
        for id in leaning {
            assert_eq!(reason(id, &server), "у Турбо", "{id}");
        }
        for id in ["voice.cloned", "brain.local", "messages", "characters.voice", "media.transcode"] {
            assert_eq!(reason(id, &away()), "Турбо не на связи", "{id}");
        }
        // «Свой» too goes down to the cloud's hearing and voice (docs/plans/speaker-alone.md).
        for id in ["mode.own", "asr.own", "voice.own"] {
            assert_eq!(reason(id, &away()), "Турбо не на связи — через облако", "{id}");
        }
        for id in ["asr.own", "voice.own", "voice.cloned", "brain.local", "messages", "characters.voice"] {
            assert_eq!(reason(id, &at(Placement::Pc)), "на этом компьютере", "{id}");
        }
        for context in [at(Placement::Pc), at(Placement::Station), server, away()] {
            for now in evaluate_all(&context) {
                let reason = now.reason.to_lowercase();
                assert!(
                    !reason.contains("компьютер не на связи")
                        && !reason.contains("домашний сервер")
                        && reason != "на компьютере"
                        && !reason.ends_with("— на компьютере"),
                    "{} in {:?}: {}",
                    now.id,
                    context.placement,
                    now.reason
                );
            }
        }
    }

    /// «Свой» on the station: the computer's while it is up, the cloud's hearing and voice otherwise
    /// (docs/plans/speaker-alone.md, stage 1); without both nothing hears or speaks, so it is off.
    #[test]
    fn svoi_on_the_station_is_the_computers_or_the_clouds() {
        let own = feature("mode.own").unwrap();
        assert_eq!(evaluate(own, &at(Placement::Pc)).state, State::Ready);
        let station = evaluate(own, &at(Placement::Station));
        assert_eq!((station.state, station.runs), (State::Ready, Some(Runs::Cloud)), "alone, through the cloud");
        let mute = evaluate(own, &Context { cloud_tts: false, ..at(Placement::Station) });
        assert_eq!((mute.state, mute.reason.as_str()), (State::Off, "Нет облачного голоса"));
        let deaf = evaluate(own, &Context { cloud_asr: false, ..at(Placement::Station) });
        assert_eq!((deaf.state, deaf.reason.as_str()), (State::Off, "Нет облачного распознавания"));
        let down = evaluate(own, &away());
        assert_eq!((down.state, down.runs), (State::Fallback, Some(Runs::Cloud)), "the helper away: the cloud");
        let nothing = evaluate(own, &Context { cloud_asr: false, ..away() });
        assert_eq!((nothing.state, nothing.reason.as_str()), (State::Off, "Турбо не на связи"));
        let up = evaluate(own, &at(Placement::StationServer));
        assert_eq!((up.state, up.runs), (State::Ready, Some(Runs::Helper)));
    }

    /// Review G2: «Свой» on the station without a helper's hearing or voice and no cloud for it would be deaf or mute
    /// — it is off, and says why; the helper away says so first; on the computer nothing is lacked.
    #[test]
    fn svoi_on_the_station_needs_the_computers_hearing_and_voice_or_the_clouds() {
        let own = feature("mode.own").unwrap();
        // The helper's hearing missing and no cloud for it (src/helpers: `svoi_lacks` says so only then).
        let deaf = Context {
            own_lacks: Some(crate::core::helpers::NO_HEARING),
            cloud_asr: false,
            ..at(Placement::StationServer)
        };
        let state = evaluate(own, &deaf);
        assert_eq!((state.state, state.reason.as_str()), (State::Off, "Нет Турбо со слухом"));
        let away = evaluate(own, &Context { helpers: away().helpers, ..deaf });
        assert_eq!(away.reason, "Турбо не на связи");
        let pc = evaluate(own, &Context { own_lacks: Some(crate::core::helpers::NO_VOICE), ..at(Placement::Pc) });
        assert_eq!(pc.state, State::Ready);
    }

    #[test]
    fn recognition_goes_down_from_the_computer_to_the_cloud_to_yandex() {
        let asr = feature("asr.own").unwrap();
        let server = at(Placement::StationServer);
        assert_eq!(evaluate(asr, &server).runs, Some(Runs::Helper));
        let asleep = away();
        let cloud = evaluate(asr, &asleep);
        assert_eq!((cloud.state, cloud.runs), (State::Fallback, Some(Runs::Cloud)));
        assert_eq!(cloud.reason, "Турбо не на связи — через облако");
        let yandex = evaluate(asr, &Context { cloud_asr: false, ..asleep });
        assert_eq!(yandex.runs, Some(Runs::Yandex));
        let nothing = evaluate(asr, &Context { cloud_asr: false, yandex: false, ..asleep });
        assert_eq!((nothing.state, nothing.reason.as_str()), (State::Off, "Турбо не на связи"));
        // On the station without a server the cloud is the first way: no fallback to speak of.
        let alone = evaluate(asr, &at(Placement::Station));
        assert_eq!((alone.state, alone.runs), (State::Ready, Some(Runs::Cloud)));
        let pc = evaluate(asr, &at(Placement::Pc));
        assert_eq!((pc.state, pc.runs, pc.reason.as_str()), (State::Ready, Some(Runs::Here), "на этом компьютере"));
    }

    /// A helper set up that does not give an ability says so — «нет помощника с …» — instead of «не на связи».
    #[test]
    fn a_helper_without_the_model_says_there_is_no_helper_with_a_model() {
        let without = |ability: Ability| {
            let rest: Vec<Ability> = Ability::V1.into_iter().filter(|other| *other != ability).collect();
            Context { helpers: giving(&rest), ..at(Placement::StationServer) }
        };
        let model = evaluate(feature("brain.local").unwrap(), &without(Ability::Model));
        assert_eq!((model.state, model.reason.as_str()), (State::Off, "Нет Турбо с моделью"));
        let hearing = evaluate(feature("asr.own").unwrap(), &without(Ability::Hearing));
        assert_eq!(
            (hearing.state, hearing.runs, hearing.reason.as_str()),
            (State::Fallback, Some(Runs::Cloud), "нет Турбо со слухом — через облако")
        );
        let voice = evaluate(feature("characters.voice").unwrap(), &without(Ability::Voice));
        assert_eq!(voice.reason, "Нет Турбо с голосом");
        let messages = evaluate(feature("messages").unwrap(), &without(Ability::Messages));
        assert_eq!(messages.reason, "Нет Турбо с сообщениями");
        // Set up for it and away: away, as before.
        let model = evaluate(feature("brain.local").unwrap(), &away());
        assert_eq!(model.reason, "Турбо не на связи");
    }

    /// A helper on the line that declares an ability but does not give it yet — its hearing still loading, its bot's
    /// contacts not read yet — is «не готов», not «не на связи»; «не настроен бот» only when it says it has none.
    #[test]
    fn a_helper_on_the_line_whose_part_is_not_ready_says_so_not_that_it_is_away() {
        let waiting = Abilities::of(&[Ability::Hearing, Ability::Messages, Ability::Model]);
        let up: Abilities = Ability::V1.into_iter().filter(|ability| !waiting.has(*ability)).collect();
        let helpers = HelperContext { up, waiting, ..at(Placement::StationServer).helpers };
        let loading = Context { helpers, ..at(Placement::StationServer) };
        let reason = |context: &Context, id: &str| evaluate(feature(id).unwrap(), context).reason;
        assert_eq!(reason(&loading, "asr.own"), "слух Турбо ещё не готов — через облако");
        assert_eq!(reason(&loading, "messages"), "Турбо ещё не готово", "its bot's contacts not read yet");
        assert_eq!(reason(&loading, "brain.local"), "Турбо ещё не готово");
        assert_eq!(reason(&loading, "voice.own"), "у Турбо");
        let no_bot = Context { helpers: HelperContext { no_bot: true, ..helpers }, ..loading };
        assert_eq!(reason(&no_bot, "messages"), "У Турбо не настроен бот Telegram");
        assert_eq!(reason(&no_bot, "brain.local"), "Турбо ещё не готово", "the bot is the messages' only");
    }

    /// The review's case: a station whose every helper the regulator keeps out (an own server at «Только дома») has
    /// them for now, not none at all — the helpers' ways stay and say «регулятор не пускает помощников», as «Свой»
    /// leaving says it, and «Свой» is there when the regulator is set aside (a stored one is kept).
    #[test]
    fn helpers_the_regulator_keeps_out_are_kept_out_for_now_not_gone() {
        let kept = HelperContext { shut: true, ..HelperContext::default() };
        for placement in [Placement::Station, Placement::StationServer] {
            let shut = Context { helpers: kept, clouds: false, ..at(placement) };
            let said = |id: &str| {
                let state = evaluate(feature(id).unwrap(), &shut);
                (state.state, state.runs, state.reason)
            };
            let off = (State::Off, None, "Регулятор не пускает Турбо".to_string());
            for id in ["mode.own", "voice.cloned", "brain.local", "messages", "characters.voice"] {
                assert_eq!(said(id), off, "{id} at {placement:?}");
            }
            let down = (State::Fallback, Some(Runs::Yandex), "регулятор не пускает Турбо — у Яндекса".to_string());
            assert_eq!((said("asr.own"), said("voice.own")), (down.clone(), down));
            let structural = Context { helpers: kept.all_up(), clouds: true, ..shut };
            assert_eq!(evaluate(feature("mode.own").unwrap(), &structural).state, State::Ready, "{placement:?}");
        }
        let server = Context { helpers: kept, ..at(Placement::StationServer) };
        assert_eq!(evaluate(feature("media.transcode").unwrap(), &server).reason, "Регулятор не пускает Турбо");
        // Nothing set up at all — no helper, no cloud hearing or voice: no way, as before the helpers.
        let bare = Context { cloud_asr: false, cloud_tts: false, ..at(Placement::Station) };
        let alone = evaluate(feature("mode.own").unwrap(), &bare);
        assert_eq!((alone.state, alone.reason.as_str()), (State::Off, "Нет облачного распознавания"));
        // A computer's own heavy part is never kept out.
        let pc = Context { helpers: kept.with_builtin(Abilities::V1), clouds: false, ..at(Placement::Pc) };
        assert!(!pc.helpers.shut);
        assert_eq!(evaluate(feature("brain.local").unwrap(), &pc).reason, "на этом компьютере");
    }

    /// «Только дома» and «Дом и моё облако» keep the clouds by keys out: their ways say so and go down to Yandex; a
    /// helper at home is never kept out.
    #[test]
    fn only_home_shuts_the_clouds_ways() {
        let shut = Context { clouds: false, ..at(Placement::Station) };
        let asr = evaluate(feature("asr.own").unwrap(), &shut);
        assert_eq!(
            (asr.state, asr.runs, asr.reason.as_str()),
            (State::Fallback, Some(Runs::Yandex), "облака закрыты регулятором — у Яндекса")
        );
        let nothing = evaluate(feature("asr.own").unwrap(), &Context { yandex: false, ..shut });
        assert_eq!((nothing.state, nothing.reason.as_str()), (State::Off, "Облака закрыты регулятором"));
        let voice = evaluate(feature("voice.own").unwrap(), &shut);
        assert_eq!((voice.runs, voice.reason.as_str()), (Some(Runs::Yandex), "облака закрыты регулятором — у Яндекса"));
        let brain = evaluate(feature("brain.cloud").unwrap(), &shut);
        assert_eq!((brain.state, brain.reason.as_str()), (State::Off, "Облака закрыты регулятором"));
        // Let out, but none set up: the old words.
        let none = evaluate(feature("asr.own").unwrap(), &Context { cloud_asr: false, ..at(Placement::Station) });
        assert_eq!(none.reason, "нет облачного распознавания — у Яндекса");
        // The helper at home hears whatever the regulator says.
        let server = Context { clouds: false, ..at(Placement::StationServer) };
        assert_eq!(evaluate(feature("asr.own").unwrap(), &server).runs, Some(Runs::Helper));
    }

    /// A helper's way runs «here» only when this computer's own heavy part gives every ability it names; a way that
    /// names none (the station leaning on its helper) is never «here» (review #24).
    #[test]
    fn a_way_runs_here_only_for_what_the_built_in_helper_gives() {
        let server = at(Placement::StationServer);
        assert_eq!(server.helpers.builtin, Abilities::NONE);
        for id in ["mode.own", "media.transcode", "stations.connect", "voice.lab", "asr.own", "brain.local"] {
            assert_eq!(evaluate(feature(id).unwrap(), &server).runs, Some(Runs::Helper), "{id}");
        }
        let pc = at(Placement::Pc);
        let asr = evaluate(feature("asr.own").unwrap(), &pc);
        assert_eq!((asr.runs, asr.reason.as_str()), (Some(Runs::Here), "на этом компьютере"));
        // Its own heavy part hears, the model is a helper's of its list.
        let mixed = Context { helpers: giving(&Ability::V1).with_builtin(Abilities::of(&[Ability::Hearing])), ..pc };
        assert_eq!(evaluate(feature("asr.own").unwrap(), &mixed).runs, Some(Runs::Here));
        let model = evaluate(feature("brain.local").unwrap(), &mixed);
        assert_eq!((model.runs, model.reason.as_str()), (Some(Runs::Helper), "у Турбо"));
        // Everything built in, and still a helper's: the way names no ability.
        let leaning = Context { helpers: HelperContext::default().with_builtin(Abilities::V1), ..server };
        let own = evaluate(feature("mode.own").unwrap(), &leaning);
        assert_eq!((own.state, own.runs), (State::Ready, Some(Runs::Helper)));
    }

    /// The station whose helper is not its computer (placement «на колонке»): «Свой» is there while that helper is on
    /// the line and gives hearing and voice; the computer's own rows stay the computer's.
    #[test]
    fn a_station_leaning_on_another_helper_offers_svoi() {
        let own = feature("mode.own").unwrap();
        let leaning = Context { helpers: giving(&[Ability::Hearing, Ability::Voice]), ..at(Placement::Station) };
        let state = evaluate(own, &leaning);
        assert_eq!((state.state, state.runs, state.reason.as_str()), (State::Ready, Some(Runs::Helper), "у Турбо"));
        let away =
            Context { helpers: HelperContext { online: false, up: Abilities::NONE, ..leaning.helpers }, ..leaning };
        let gone = evaluate(own, &away);
        assert_eq!((gone.state, gone.runs), (State::Fallback, Some(Runs::Cloud)), "the helper away: the cloud");
        let gone = evaluate(own, &Context { cloud_asr: false, ..away });
        assert_eq!((gone.state, gone.reason.as_str()), (State::Off, "Турбо не на связи"));
        let deaf = evaluate(own, &Context { own_lacks: Some(crate::core::helpers::NO_HEARING), ..leaning });
        assert_eq!((deaf.state, deaf.runs), (State::Fallback, Some(Runs::Cloud)), "the cloud hears instead");
        let deaf =
            evaluate(own, &Context { own_lacks: Some(crate::core::helpers::NO_HEARING), cloud_asr: false, ..leaning });
        assert_eq!((deaf.state, deaf.reason.as_str()), (State::Off, "Нет Турбо со слухом"));
        let bare = Context { cloud_asr: false, cloud_tts: false, ..at(Placement::Station) };
        let alone = evaluate(own, &bare);
        assert_eq!(
            (alone.state, alone.reason.as_str()),
            (State::Off, "Нет облачного распознавания"),
            "no helper, no cloud: as before"
        );
        assert_eq!(evaluate(feature("asr.own").unwrap(), &leaning).runs, Some(Runs::Helper));
        for id in ["stations.connect", "voice.lab", "media.transcode"] {
            assert!(!evaluate(feature(id).unwrap(), &leaning).on(), "{id} stays the computer's");
        }
    }

    #[test]
    fn the_helpers_context_is_said_by_the_abilities_names() {
        let set = HelperContext {
            online: false,
            configured: Abilities::of(&[Ability::Hearing, Ability::Voice, Ability::Model]),
            waiting: Abilities::of(&[Ability::Voice, Ability::Model]),
            ..HelperContext::default()
        };
        assert!(set.set_up() && !HelperContext::default().set_up());
        assert_eq!(set.all_up(), HelperContext { online: true, up: set.configured, waiting: Abilities::NONE, ..set });
        assert_eq!(HelperContext::default().all_up(), HelperContext::default(), "nothing set up: nobody on the line");
        assert_eq!(HelperContext::default().with_builtin(Abilities::NONE), HelperContext::default());
        let built = set.with_builtin(Abilities::of(&[Ability::Voice]));
        assert_eq!(
            serde_json::to_value(built).unwrap(),
            serde_json::json!({
                "online": true, "configured": ["hearing", "voice", "model"], "up": ["voice"], "waiting": ["model"],
                "builtin": ["voice"]
            }),
            "what it gives itself is not waited for"
        );
        // Kept out by the regulator: there, on the line when it is set aside; said only when so.
        let kept = HelperContext { shut: true, ..HelperContext::default() };
        assert!(kept.there() && !kept.set_up() && kept.all_up().online);
        let no_bot = HelperContext { no_bot: true, waiting: Abilities::of(&[Ability::Messages]), ..kept };
        assert_eq!(
            serde_json::to_value(no_bot).unwrap(),
            serde_json::json!({
                "online": false, "configured": [], "up": [], "waiting": ["messages"], "noBot": true, "builtin": [],
                "shut": true
            })
        );
        let built = no_bot.with_builtin(Abilities::V1);
        assert!(!built.shut && !built.no_bot && built.waiting.is_empty(), "its own heavy part gives them: {built:?}");
    }

    /// The reasons are static texts translated when said (`core::tx`); the catalogs' own test checks only `tr!`. Every
    /// word of the helpers' ways has its English, the regulator's included.
    #[test]
    fn every_word_of_the_helpers_ways_has_its_english() {
        let english = |text: &'static str| crate::locale::text::translated(text, Some("en"));
        let mut words = vec![
            HELPER_AWAY,
            HELPERS_SHUT,
            HELPER_NO_BOT,
            CLOUDS_SHUT,
            Context::best(Placement::StationServer).place(Runs::Helper),
        ];
        words.extend(Ability::ALL.into_iter().map(no_helper_with));
        words.extend(Ability::ALL.into_iter().map(helper_not_ready));
        for word in words {
            assert_ne!(english(word), word, "«{word}» has no English line");
        }
    }

    #[test]
    fn the_panels_list_of_ids_is_the_registrys() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../panel/src/availability-ids.ts");
        let expected = panel_ids();
        if std::env::var_os("DUOWAVE_WRITE_PANEL_IDS").is_some() {
            std::fs::write(&path, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "panel/src/availability-ids.ts is not the registry's: DUOWAVE_WRITE_PANEL_IDS=1 cargo test -p duowave availability"
        );
    }
}
