//! What works where — one registry of the gateway's features (docs/plans/station-placement.md, section 1). A feature
//! is described once: the ways it can run, in order of preference, and what each way needs. The gateway, the panel
//! of the computer and the panel of the station all read it from here: whether a feature is on now, where it runs
//! (here, on the home server, in the cloud, at Yandex's) and why not. A new feature is a row in `FEATURES` (and one
//! `available('…')` in the panel where it is to be greyed out).
//!
//! Not to be confused with the routes' capabilities (`skills/capabilities.rs`): those are what the station can do for
//! the household; these are what this gateway can do where it runs.

use serde::{Deserialize, Serialize};

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
            Placement::Pc => "Колонка говорит со шлюзом на компьютере. Всё своё: распознавание, голос, модели, звонки.",
            Placement::Station => {
                "Шлюз на самой колонке: работает без компьютера, с облачными подключениями или Яндексом. Без своих моделей и звонков."
            }
            Placement::StationServer => {
                "Шлюз на колонке, а тяжёлое — распознавание, свой голос, модели — он берёт у компьютера, пока тот на связи. Компьютер спит — работает через облако или Яндекс."
            }
        }
    }

    pub fn risk(self) -> &'static str {
        match self {
            Placement::Pc => "Компьютер спит или выключен — колонка молчит.",
            Placement::Station => {
                "Мало места и памяти на колонке; служба перезапускает шлюз, страховка возвращает Яндекс."
            }
            Placement::StationServer => "Без компьютера — урезанно; «Своё» только пока компьютер на связи.",
        }
    }
}

/// Where a feature runs now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Runs {
    /// In this gateway: on the computer, or on the station.
    Here,
    /// On the home server (the computer the station's gateway leans on).
    HomeServer,
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
            Group::Brain => "Мозг",
            Group::Skills => "Навыки",
            Group::House => "Дом",
            Group::System => "Система",
        }
    }
}

/// What a way needs. `PcBuild` and `At` say whether the way exists in this placement at all; the rest whether it
/// works right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// The computer's build: local model servers, the voice lab, calls, SSH, MCP programs are compiled in.
    PcBuild,
    /// One of these placements.
    At(&'static [Placement]),
    /// The home server answers now.
    HomeServer,
    /// A recognizer in the cloud is set up.
    CloudAsr,
    /// A voice in the cloud is set up.
    CloudTts,
    /// Yandex may serve: the mode is not «Свой».
    Yandex,
    /// This much of the plugins' memory is free here (src/plugins/budget.rs): on the station it is short.
    Memory(u64),
    /// Streams can be transcoded here: a transcoder (ffmpeg) on a computer, or on the station by the owner's flag
    /// (MEDIA_TRANSCODE_ON_STATION) — it loads the station's weak processor.
    Transcode,
    /// «Свой» on the station hears and speaks: the computer gives it hearing and voice («Что даю колонке»), or the
    /// cloud stands in for the part it does not — without Yandex nothing else would.
    OwnParts,
    /// This gateway hears MIDI keyboards: a computer's build with `midi`, and not `MIDI=0` (src/midi).
    Midi,
}

impl Need {
    /// Whether the way exists here at all, as opposed to working right now.
    fn structural(self) -> bool {
        matches!(self, Need::PcBuild | Need::At(_) | Need::Transcode | Need::Midi)
    }
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

/// Why «Свой» is not on the station: the switch refuses it with this, the panel says it.
pub const OWN_ON_STATION: &str = "«Своё» на колонке не включить: без Яндекса слух, голос и модели нужны \
свои, а для них нужен компьютер с DuoWave. На колонке — «Яндекс» или «Вместе» с облачными подключениями.";

pub const FEATURES: &[Feature] = &[
    Feature {
        id: "mode.relay",
        title: "Режим «Яндекс»",
        group: Group::Modes,
        ways: &[Way { runs: Runs::Yandex, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "mode.gateway",
        title: "Режим «Вместе»",
        group: Group::Modes,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "mode.own",
        title: "Режим «Своё»",
        group: Group::Modes,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way {
                runs: Runs::HomeServer,
                needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer, Need::OwnParts],
            },
        ],
        absent: OWN_ON_STATION,
        table: [yes("да"), no("нет: нужен свой сервер"), partial("пока компьютер на связи")],
    },
    Feature {
        id: "asr.own",
        title: "Своё распознавание",
        group: Group::Voice,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
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
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
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
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
        ],
        absent: "Голоса из лаборатории звучат только с компьютера: их модели там.",
        table: [yes("да"), no("нет"), partial("через компьютер")],
    },
    Feature {
        id: "brain.cloud",
        title: "Мозг: облачные модели",
        group: Group::Brain,
        ways: &[Way { runs: Runs::Cloud, needs: &[] }],
        absent: "",
        table: [yes("да"), yes("да"), yes("да")],
    },
    Feature {
        id: "brain.local",
        title: "Мозг: модель на компьютере",
        group: Group::Brain,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
        ],
        absent: "Своя модель работает на компьютере: на колонке ей не хватит ни памяти, ни процессора.",
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
    // The bot is the calls module's, the computer's build only: on the station «передай …» goes through the
    // computer's (src/home_server/messenger.rs).
    Feature {
        id: "messages",
        title: "Сообщения в Telegram: «передай …» и входящие",
        group: Group::House,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
        ],
        absent: "Сообщения в Telegram уходят через бота компьютера: в сборке для колонки его нет.",
        table: [yes("да"), no("нет"), partial("через компьютер")],
    },
    Feature {
        id: "calls",
        title: "Звонки: страница, SIP, LiveKit",
        group: Group::House,
        ways: &[Way { runs: Runs::Here, needs: &[Need::PcBuild] }],
        absent: "Звонки — только в шлюзе на компьютере: страница звонков, SIP и LiveKit там.",
        table: [yes("да"), no("нет"), no("нет (позже)")],
    },
    Feature {
        id: "stations.connect",
        title: "Поиск и подключение колонок",
        group: Group::System,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER)] },
        ],
        absent: "Колонки ищет и подключает шлюз на компьютере: в сборке для колонки нет SSH.",
        table: [yes("да"), no("нет"), partial("с компьютера")],
    },
    Feature {
        id: "voice.lab",
        title: "Голосовая лаборатория",
        group: Group::Voice,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER)] },
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
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::PcBuild] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
        ],
        absent: "Голоса персонажей звучат с компьютера: их модели там. На колонке говорит запасной голос.",
        table: [yes("да"), no("нет: запасной голос"), partial("через компьютер")],
    },
    Feature {
        id: "plugins",
        title: "Приложения",
        group: Group::Skills,
        ways: &[Way { runs: Runs::Here, needs: &[] }],
        absent: "",
        table: [yes("да"), partial("в пределах памяти колонки"), partial("в пределах памяти колонки")],
    },
    Feature {
        id: "plugins.apps",
        title: "Приложения (Spotify и другие)",
        group: Group::Skills,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::Memory(APP_MEMORY_MB)] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
        ],
        absent: "",
        table: [
            yes("да"),
            partial("если хватит памяти колонки"),
            partial("на компьютере; без него — если хватит памяти"),
        ],
    },
    Feature {
        id: "media.transcode",
        title: "Перекодирование потоков (AAC → MP3)",
        group: Group::Skills,
        ways: &[
            Way { runs: Runs::Here, needs: &[Need::Transcode] },
            Way { runs: Runs::HomeServer, needs: &[Need::At(ON_STATION_WITH_SERVER), Need::HomeServer] },
        ],
        absent: "На колонке перекодирование выключено: большая нагрузка на её процессор (MEDIA_TRANSCODE_ON_STATION).",
        table: [yes("да"), no("нет — большая нагрузка"), partial("на компьютере")],
    },
    Feature {
        id: "plugins.midi",
        title: "MIDI-клавиатура для приложений",
        group: Group::Skills,
        ways: &[Way { runs: Runs::Here, needs: &[Need::Midi] }],
        absent: "Клавиатуру слышит шлюз на компьютере: в сборке колонки MIDI нет. В «колонка + сервер» — пульт помощника на компьютере.",
        table: [yes("да, клавиатура — к компьютеру"), no("нет: в сборке колонки MIDI нет"), no("у шлюза колонки нет; у пульта помощника на компьютере — да")],
    },
    Feature {
        id: "offline",
        title: "Работает, когда компьютер спит",
        group: Group::System,
        ways: &[Way { runs: Runs::Here, needs: &[Need::At(&[Placement::Station, Placement::StationServer])] }],
        absent: "Шлюз на компьютере: компьютер спит — колонка молчит.",
        table: [no("нет"), yes("да"), partial("да, урезанно")],
    },
];

/// What an app takes of the station's memory, at least (librespot: 20–30 MB).
pub const APP_MEMORY_MB: u64 = 30;

pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|feature| feature.id == id)
}

/// The home server as the station's gateway sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HomeServer {
    /// None set up.
    None,
    Up,
    Down,
}

/// What the evaluation reads: the build, the placement, the home server, the cloud, the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub pc_build: bool,
    pub placement: Placement,
    pub home_server: HomeServer,
    pub cloud_asr: bool,
    pub cloud_tts: bool,
    /// Not «Свой».
    pub yandex: bool,
    /// The plugins' free memory here, when it is counted (on the station); none: not short.
    pub memory_free_mb: Option<u64>,
    /// A transcoder is here (ffmpeg found).
    pub transcoder: bool,
    /// MEDIA_TRANSCODE_ON_STATION: the owner lets the station transcode.
    pub transcode_on_station: bool,
    /// What «Свой» would lack on the station as the computer gives now: its hearing or its voice not taken and no
    /// cloud for it (src/home_server, `svoi_lacks`) — said as the reason; none: nothing.
    pub own_lacks: Option<&'static str>,
    /// This gateway hears MIDI keyboards (src/midi).
    pub midi: bool,
}

impl Context {
    /// The best a placement can do: for the comparison of placements.
    pub fn best(placement: Placement) -> Context {
        Context {
            pc_build: placement == Placement::Pc,
            placement,
            home_server: if placement == Placement::StationServer { HomeServer::Up } else { HomeServer::None },
            cloud_asr: true,
            cloud_tts: true,
            yandex: true,
            memory_free_mb: None,
            transcoder: true,
            transcode_on_station: false,
            own_lacks: None,
            midi: placement == Placement::Pc,
        }
    }

    fn meets(&self, need: Need) -> bool {
        match need {
            Need::PcBuild => self.pc_build,
            Need::At(placements) => placements.contains(&self.placement),
            Need::HomeServer => self.home_server == HomeServer::Up,
            Need::CloudAsr => self.cloud_asr,
            Need::CloudTts => self.cloud_tts,
            Need::Yandex => self.yandex,
            Need::Memory(mb) => self.memory_free_mb.is_none_or(|free| free >= mb),
            Need::Transcode => self.transcoder && (self.placement == Placement::Pc || self.transcode_on_station),
            Need::OwnParts => self.own_lacks.is_none(),
            Need::Midi => self.midi,
        }
    }

    /// Why a need is not met, for people.
    fn why(&self, need: Need) -> &'static str {
        match need {
            Need::PcBuild | Need::At(_) => "не в этом размещении",
            Need::HomeServer if self.home_server == HomeServer::None => "домашний сервер не настроен",
            Need::HomeServer => "компьютер не на связи",
            Need::CloudAsr => "нет облачного распознавания",
            Need::CloudTts => "нет облачного голоса",
            Need::Yandex => "в «Своём» Яндекс выключен",
            Need::Memory(_) => "на колонке не хватит памяти",
            Need::Transcode => "нет перекодировщика",
            Need::OwnParts => self.own_lacks.unwrap_or("компьютер не даёт колонке слух или голос"),
            Need::Midi => "в этой сборке MIDI нет или он выключен (MIDI=0)",
        }
    }

    /// Where a way runs, for people.
    fn place(&self, runs: Runs) -> &'static str {
        match runs {
            Runs::Here if self.placement == Placement::Pc => "на этом компьютере",
            Runs::Here => "на колонке",
            Runs::HomeServer => "на компьютере",
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
    /// A later way works: on the way down («компьютер не на связи — через облако»).
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
    let here: Vec<&Way> = feature
        .ways
        .iter()
        .filter(|way| way.needs.iter().filter(|need| need.structural()).all(|need| context.meets(*need)))
        .collect();
    let Some(meant) = here.first() else {
        return state(State::Off, None, super::tx(feature.absent));
    };
    let unmet = |way: &Way| way.needs.iter().copied().find(|need| !context.meets(*need));
    match here.iter().enumerate().find(|(_, way)| unmet(way).is_none()) {
        Some((0, way)) => state(State::Ready, Some(way.runs), super::tx(context.place(way.runs))),
        Some((_, way)) => {
            let why = unmet(meant).map(|need| super::tx(context.why(need))).unwrap_or_default();
            state(State::Fallback, Some(way.runs), format!("{why} — {}", super::tx(context.place(way.runs))))
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

    #[test]
    fn svoi_is_the_computers_on_the_station_only_with_the_server_up() {
        let own = feature("mode.own").unwrap();
        assert_eq!(evaluate(own, &at(Placement::Pc)).state, State::Ready);
        let station = evaluate(own, &at(Placement::Station));
        assert_eq!((station.state, station.reason.as_str()), (State::Off, OWN_ON_STATION));
        let server = Context { home_server: HomeServer::Down, ..at(Placement::StationServer) };
        let down = evaluate(own, &server);
        assert_eq!((down.state, down.reason.as_str()), (State::Off, "Компьютер не на связи"));
        let up = evaluate(own, &at(Placement::StationServer));
        assert_eq!((up.state, up.runs), (State::Ready, Some(Runs::HomeServer)));
    }

    /// Review G2: «Свой» on the station without the computer's hearing or voice and no cloud for it would be deaf or
    /// mute — it is off, and says why; the computer away says so first; on the computer nothing is lacked.
    #[test]
    fn svoi_on_the_station_needs_the_computers_hearing_and_voice_or_the_clouds() {
        let own = feature("mode.own").unwrap();
        let hearing_off = Some("слух компьютера выключен");
        let deaf = Context { own_lacks: hearing_off, ..at(Placement::StationServer) };
        let state = evaluate(own, &deaf);
        assert_eq!((state.state, state.reason.as_str()), (State::Off, "Слух компьютера выключен"));
        let away = evaluate(own, &Context { home_server: HomeServer::Down, ..deaf });
        assert_eq!(away.reason, "Компьютер не на связи");
        let pc = evaluate(own, &Context { own_lacks: Some("голос компьютера выключен"), ..at(Placement::Pc) });
        assert_eq!(pc.state, State::Ready);
    }

    #[test]
    fn recognition_goes_down_from_the_computer_to_the_cloud_to_yandex() {
        let asr = feature("asr.own").unwrap();
        let server = at(Placement::StationServer);
        assert_eq!(evaluate(asr, &server).runs, Some(Runs::HomeServer));
        let asleep = Context { home_server: HomeServer::Down, ..server };
        let cloud = evaluate(asr, &asleep);
        assert_eq!((cloud.state, cloud.runs), (State::Fallback, Some(Runs::Cloud)));
        assert_eq!(cloud.reason, "компьютер не на связи — через облако");
        let yandex = evaluate(asr, &Context { cloud_asr: false, ..asleep });
        assert_eq!(yandex.runs, Some(Runs::Yandex));
        let nothing = evaluate(asr, &Context { cloud_asr: false, yandex: false, ..asleep });
        assert_eq!((nothing.state, nothing.reason.as_str()), (State::Off, "Компьютер не на связи"));
        // On the station without a server the cloud is the first way: no fallback to speak of.
        let alone = evaluate(asr, &at(Placement::Station));
        assert_eq!((alone.state, alone.runs), (State::Ready, Some(Runs::Cloud)));
        let pc = evaluate(asr, &at(Placement::Pc));
        assert_eq!((pc.state, pc.reason.as_str()), (State::Ready, "на этом компьютере"));
    }

    #[test]
    fn the_panels_list_of_ids_is_the_registrys() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../panel/src/availability-ids.ts");
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
