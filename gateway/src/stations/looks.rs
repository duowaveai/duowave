//! How a station looks in the panel: its family (from the model it reports), the variant and the colour. At sync the
//! station names its model (`device_model`, a firmware code name: `plum` — the Lite 2, the whole list in FAMILIES
//! below) and its colour (`device_color`: «blue»). Whether a Lite 2 has the clock it does not say, but some colours
//! are sold in one variant only. What the data settles is not offered for a choice; the rest a person picks, and
//! until then the first variant and its first colour stand.
//!
//! The codes are what the station itself says at sync (mDNS `platform` of a not-yet-known station is the same word):
//! taken from the devices we hold (`yandex-station-extended/docs/devices`) and, for the models we do not, from the
//! platform list the YandexStation integration reads out of the cloud device list. `bork` is a guess: the model is
//! too new for its code to be public — the day one syncs, its real word lands in the journal and this table.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LookVariant {
    pub id: &'static str,
    /// «Лайт 2 с часами».
    pub title: &'static str,
    /// The word on the picker's segment: «С часами».
    pub label: &'static str,
    pub clock: bool,
    /// The colours it is sold in, the default first (the panel's drawing knows them by these ids).
    pub colors: &'static [&'static str],
}

struct Family {
    id: &'static str,
    title: &'static str,
    variants: &'static [LookVariant],
    /// This firmware's own words for its colours → ours, looked at before REPORTED_COLORS (plum calls its coral
    /// «orange»; everywhere else orange is orange).
    reports: &'static [(&'static str, &'static str)],
}

/// Firmware code names → families. A new model: its variants here, its drawing in panel/src/stations.
/// Retail names and the firmware's disagree twice — `yandexstation_2` is the Max, and the retail Station 2 answers
/// to `yandexmidi`, while the retail Midi is `cucumber`. The colours are the official ones of each model.
#[rustfmt::skip]
const FAMILIES: &[(&str, Family)] = &[
    // Лайт 2 (2024): с часами — синий, фиолетовый, розовый, графит; без — ещё зелёный и коралловый.
    ("plum", Family { id: "lite2", title: "Лайт 2",
        variants: &[
            LookVariant { id: "clock", title: "Лайт 2 с часами", label: "С часами", clock: true,
                colors: &["azure", "lilac", "pink", "graphite"] },
            LookVariant { id: "plain", title: "Лайт 2", label: "Без часов", clock: false,
                colors: &["azure", "lilac", "green", "pink", "coral"] },
        ],
        reports: &[("orange", "coral")],
    }),
    // Лайт (2021): ультрафиолет, бирюзовая, красная, лимонная, коралловая, капучино — все без часов.
    ("yandexmicro", Family { id: "lite1", title: "Лайт",
        variants: &[LookVariant { id: "one", title: "Лайт", label: "Без часов", clock: false,
            colors: &["lilac", "teal", "red", "yellow", "pink", "sand"] }],
        reports: &[],
    }),
    // Мини (2019): графит, белая, синяя, фиолетовая, красная.
    ("yandexmini", Family { id: "mini1", title: "Мини",
        variants: &[LookVariant { id: "one", title: "Мини", label: "Без часов", clock: false,
            colors: &["graphite", "white", "azure", "lilac", "red"] }],
        reports: &[],
    }),
    // Мини 2 (2021): графитовый, фиолетовый, красный гранат, синий, зелёный, розовый.
    ("yandexmini_2", Family { id: "mini2", title: "Мини 2",
        variants: &[
            LookVariant { id: "clock", title: "Мини 2 с часами", label: "С часами", clock: true,
                colors: &["graphite", "lilac", "red", "azure", "green", "pink"] },
            LookVariant { id: "plain", title: "Мини 2", label: "Без часов", clock: false,
                colors: &["graphite", "lilac", "red", "azure", "green", "pink"] },
        ],
        reports: &[],
    }),
    // Мини 3 (2024): графит, лиловый, бирюзовый, зелёный, красный; обе версии часов отвечают одним кодом.
    ("bergamot", Family { id: "mini3", title: "Мини 3",
        variants: &[
            LookVariant { id: "clock", title: "Мини 3 с часами", label: "С часами", clock: true,
                colors: &["graphite", "lilac", "teal", "green", "red"] },
            LookVariant { id: "plain", title: "Мини 3", label: "Без часов", clock: false,
                colors: &["graphite", "lilac", "teal", "green", "red"] },
        ],
        reports: &[],
    }),
    // Мини 3 Про (2025): графитовый, зелёный, красный.
    ("pickle", Family { id: "mini3pro", title: "Мини 3 Про",
        variants: &[LookVariant { id: "one", title: "Мини 3 Про", label: "С часами", clock: true,
            colors: &["graphite", "green", "red"] }],
        reports: &[],
    }),
    // Станция (2018): только чёрная.
    ("yandexstation", Family { id: "station", title: "Станция",
        variants: &[LookVariant { id: "one", title: "Станция", label: "Без часов", clock: false,
            colors: &["graphite"] }],
        reports: &[],
    }),
    // Макс (2020, и ревизия с Zigbee 2023 — тот же код): графит, бежевый, изумруд, красный, фиолетовый, бирюзовый.
    ("yandexstation_2", Family { id: "max", title: "Макс",
        variants: &[LookVariant { id: "one", title: "Макс", label: "С дисплеем", clock: true,
            colors: &["graphite", "sand", "green", "red", "lilac", "teal"] }],
        reports: &[],
    }),
    // Станция 2 (2022): антрацит, кобальт, медный, песочный, красный рубин.
    ("yandexmidi", Family { id: "station2", title: "Станция 2",
        variants: &[LookVariant { id: "one", title: "Станция 2", label: "С дисплеем", clock: true,
            colors: &["graphite", "azure", "copper", "sand", "red"] }],
        reports: &[],
    }),
    // Миди (2023): графит, изумруд, оранжевый, малиновый.
    ("cucumber", Family { id: "midi", title: "Миди",
        variants: &[LookVariant { id: "one", title: "Миди", label: "Без часов", clock: false,
            colors: &["graphite", "green", "orange", "red"] }],
        reports: &[("raspberry", "red")],
    }),
    // Дуо Макс (2023): чёрный, зелёный.
    ("chiron", Family { id: "duomax", title: "Дуо Макс",
        variants: &[LookVariant { id: "one", title: "Дуо Макс", label: "С экраном", clock: true,
            colors: &["graphite", "green"] }],
        reports: &[],
    }),
    // Станция 3 (2025): серая.
    ("orion", Family { id: "station3", title: "Станция 3",
        variants: &[LookVariant { id: "one", title: "Станция 3", label: "С дисплеем", clock: true,
            colors: &["graphite"] }],
        reports: &[],
    }),
    // Стрит (2025): оранжевый, чёрный, зелёный, фиолетовый.
    ("mango", Family { id: "street", title: "Стрит",
        variants: &[LookVariant { id: "one", title: "Стрит", label: "Без часов", clock: false,
            colors: &["orange", "graphite", "green", "lilac"] }],
        reports: &[],
    }),
    // Станция BORK (2025): светлая (глянцевая шапка, светлая ткань); код не публиковался — до синхронизации не проверишь.
    ("bork", Family { id: "bork", title: "Станция BORK",
        variants: &[LookVariant { id: "one", title: "Станция BORK", label: "С экраном", clock: true,
            colors: &["white"] }],
        reports: &[],
    }),
];

/// The station's device_color → ours. A name not here settles nothing: then the colour is a person's choice.
#[rustfmt::skip]
/// Families sold with stickers in the box (the Lite 2's cats): a person may say whether theirs wears one. The station
/// does not know it, so the look only offers the choice; the panel draws the sticker on the front.
const STICKERED: &[&str] = &["lite2"];

const REPORTED_COLORS: &[(&str, &str)] = &[
    ("blue", "azure"),
    ("azure", "azure"),
    ("cobalt", "azure"),
    ("purple", "lilac"),
    ("violet", "lilac"),
    ("lilac", "lilac"),
    ("pink", "pink"),
    ("green", "green"),
    ("emerald", "green"),
    ("coral", "coral"),
    ("orange", "orange"),
    ("red", "red"),
    ("ruby", "red"),
    ("garnet", "red"),
    ("crimson", "red"),
    ("yellow", "yellow"),
    ("lemon", "yellow"),
    ("teal", "teal"),
    ("turquoise", "teal"),
    ("mint", "teal"),
    ("sand", "sand"),
    ("beige", "sand"),
    ("gold", "sand"),
    ("amber", "sand"),
    ("cappuccino", "sand"),
    ("copper", "copper"),
    ("white", "white"),
    ("milk", "white"),
    ("ivory", "white"),
    ("black", "graphite"),
    ("graphite", "graphite"),
    ("anthracite", "graphite"),
    ("gray", "graphite"),
    ("grey", "graphite"),
];

/// Where a part of the look comes from: the station's own data, a person's choice, or the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LookSource {
    Station,
    Chosen,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationLook {
    pub family: &'static str,
    /// The family's name: «Лайт 2».
    pub title: &'static str,
    /// The variant and the colour in force.
    pub model: &'static str,
    pub color: &'static str,
    pub clock: bool,
    pub model_from: LookSource,
    pub color_from: LookSource,
    /// What may be chosen: the variants the data leaves open (one — settled), the colours of the variant in force.
    pub variants: Vec<&'static LookVariant>,
    pub colors: Vec<&'static str>,
    /// Whether it wears a sticker: None — the model has none to wear; the choice itself is the station record's.
    pub sticker: Option<bool>,
}

/// What the station said of itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reported<'a> {
    pub device_model: Option<&'a str>,
    pub device_color: Option<&'a str>,
}

/// What a person chose.
#[derive(Debug, Clone, Copy, Default)]
pub struct Chosen<'a> {
    pub model: Option<&'a str>,
    pub color: Option<&'a str>,
}

/// A value JavaScript would take as true: set and not empty.
fn given(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// The look in force; None for a model the panel has no drawing for.
pub fn look_of(reported: Reported, chosen: Chosen) -> Option<StationLook> {
    let code = given(reported.device_model)?.trim().to_lowercase();
    let family = &FAMILIES.iter().find(|(name, _)| *name == code)?.1;
    let said = given(reported.device_color).and_then(|color| {
        let color = color.trim().to_lowercase();
        family.reports.iter().chain(REPORTED_COLORS).find(|(name, _)| *name == color).map(|(_, ours)| *ours)
    });
    let fitting: Vec<&'static LookVariant> = match said {
        Some(said) => family.variants.iter().filter(|variant| variant.colors.contains(&said)).collect(),
        None => Vec::new(),
    };
    // A colour none of the variants is sold in settles nothing: neither the colour nor the variant.
    let sure = if fitting.is_empty() { None } else { said };
    let variants = if fitting.is_empty() { family.variants.iter().collect() } else { fitting };
    let picked = variants.iter().copied().find(|variant| Some(variant.id) == chosen.model);
    let variant = if variants.len() == 1 { variants[0] } else { picked.unwrap_or(variants[0]) };
    let color = match sure {
        Some(sure) => sure,
        None => given(chosen.color)
            .and_then(|wanted| variant.colors.iter().copied().find(|color| *color == wanted))
            .unwrap_or(variant.colors[0]),
    };
    let model_from = if variants.len() == 1 {
        LookSource::Station
    } else if picked.is_some() {
        LookSource::Chosen
    } else {
        LookSource::Default
    };
    let color_from = if sure.is_some() {
        LookSource::Station
    } else if Some(color) == chosen.color {
        LookSource::Chosen
    } else {
        LookSource::Default
    };
    Some(StationLook {
        family: family.id,
        title: family.title,
        model: variant.id,
        color,
        clock: variant.clock,
        model_from,
        color_from,
        colors: match sure {
            Some(sure) => vec![sure],
            None => variant.colors.to_vec(),
        },
        variants,
        sticker: STICKERED.contains(&family.id).then_some(false),
    })
}

/// Why a choice cannot be taken (for a 400), or None when it can: checked against what the data leaves open.
pub fn look_problem(reported: Reported, next: Chosen) -> Option<String> {
    let Some(look) = look_of(reported, Chosen::default()) else {
        return (given(next.model).is_some() || given(next.color).is_some())
            .then(|| "Вид этой модели колонки пока не выбрать.".to_string());
    };
    if let Some(model) = given(next.model) {
        if look.variants.len() == 1 && model != look.model {
            return Some("Модель колонка сообщила сама.".into());
        }
        if !look.variants.iter().any(|variant| variant.id == model) {
            let ids: Vec<&str> = look.variants.iter().map(|variant| variant.id).collect();
            return Some(format!("Модель: {}.", ids.join(" или ")));
        }
    }
    if let Some(color) = given(next.color) {
        let wanted = next.model.unwrap_or(look.model);
        let variant = look.variants.iter().find(|entry| entry.id == wanted).unwrap_or(&look.variants[0]);
        if look.color_from == LookSource::Station && color != look.color {
            return Some("Цвет колонка сообщила сама.".into());
        }
        if !variant.colors.contains(&color) {
            return Some(format!("Цвет: {}.", variant.colors.join(", ")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lite2(device_color: Option<&str>) -> Reported<'_> {
        Reported { device_model: Some("plum"), device_color }
    }

    const NONE: Chosen<'static> = Chosen { model: None, color: None };

    fn ids(variants: &[&LookVariant]) -> Vec<&'static str> {
        variants.iter().map(|variant| variant.id).collect()
    }

    #[test]
    fn takes_the_colour_from_the_station_and_leaves_the_clock_to_a_person_when_both_variants_come_in_it() {
        let look = look_of(lite2(Some("blue")), NONE).unwrap();
        assert_eq!(look.family, "lite2");
        assert_eq!((look.model, look.clock, look.color), ("clock", true, "azure"));
        assert_eq!((look.model_from, look.color_from), (LookSource::Default, LookSource::Station));
        assert_eq!(ids(&look.variants), ["clock", "plain"]);
        assert_eq!(look.colors, ["azure"]);
        let chosen = look_of(lite2(Some("blue")), Chosen { model: Some("plain"), color: Some("pink") }).unwrap();
        assert_eq!(
            (chosen.model, chosen.clock, chosen.model_from, chosen.color),
            ("plain", false, LookSource::Chosen, "azure")
        );
    }

    #[test]
    fn settles_the_variant_when_only_one_is_sold_in_the_colour() {
        let green = look_of(lite2(Some("green")), Chosen { model: Some("clock"), color: None }).unwrap();
        assert_eq!(
            (green.model, green.clock, green.model_from, green.color),
            ("plain", false, LookSource::Station, "green")
        );
        let black = look_of(lite2(Some("black")), NONE).unwrap();
        assert_eq!((black.model, black.model_from), ("clock", LookSource::Station));
        assert_eq!((black.color, black.color_from), ("graphite", LookSource::Station));
    }

    #[test]
    fn leaves_both_to_a_person_when_the_colour_is_unknown_the_first_variant_and_colour_standing_until_then() {
        let look = look_of(lite2(None), NONE).unwrap();
        assert_eq!((look.model, look.color), ("clock", "azure"));
        assert_eq!((look.model_from, look.color_from), (LookSource::Default, LookSource::Default));
        assert_eq!(look.colors, ["azure", "lilac", "pink", "graphite"]);
        let chosen = look_of(lite2(Some("yellow")), Chosen { model: Some("plain"), color: Some("coral") }).unwrap();
        assert_eq!((chosen.model, chosen.color, chosen.color_from), ("plain", "coral", LookSource::Chosen));
        // A colour the variant is not sold in falls back to its first.
        let fallback = look_of(lite2(None), Chosen { model: Some("clock"), color: Some("green") }).unwrap();
        assert_eq!((fallback.color, fallback.color_from), ("azure", LookSource::Default));
    }

    #[test]
    fn knows_every_family_the_panel_draws_and_settles_a_single_variant_model_from_its_colour() {
        // Every entry of the table is a family the panel has a drawing for, and its first colour is its default.
        for (code, family) in FAMILIES {
            let look = look_of(Reported { device_model: Some(code), device_color: None }, NONE).unwrap();
            assert_eq!(look.family, family.id, "{code}");
            assert_eq!(look.title, family.title, "{code}");
            assert_eq!(look.variants.len(), family.variants.len(), "{code}");
            let first = &family.variants[0];
            assert_eq!((look.model, look.color), (first.id, first.colors[0]), "{code}");
        }
        // The Max: one variant, so its settled colour settles the model too — no choice to offer.
        let max = look_of(Reported { device_model: Some("yandexstation_2"), device_color: Some("red") }, NONE).unwrap();
        assert_eq!(
            (max.family, max.model, max.color, max.clock, max.model_from, max.color_from),
            ("max", "one", "red", true, LookSource::Station, LookSource::Station)
        );
        assert_eq!(max.variants.len(), 1);
        // The retail Station 2 answers to yandexmidi (the Midi is cucumber): both draw their own families.
        let station2 = look_of(Reported { device_model: Some("yandexmidi"), device_color: Some("red") }, NONE).unwrap();
        assert_eq!((station2.family, station2.color), ("station2", "red"));
        let midi = look_of(Reported { device_model: Some("cucumber"), device_color: Some("raspberry") }, NONE).unwrap();
        assert_eq!((midi.family, midi.color), ("midi", "red"));
        let mini2 = look_of(Reported { device_model: Some("yandexmini_2"), device_color: None }, NONE).unwrap();
        assert_eq!((mini2.family, ids(&mini2.variants).as_slice()), ("mini2", ["clock", "plain"].as_slice()));
    }

    #[test]
    fn plum_calls_its_coral_orange_everywhere_else_orange_is_orange() {
        let coral = look_of(lite2(Some("orange")), NONE).unwrap();
        assert_eq!(coral.color, "coral");
        let street = look_of(Reported { device_model: Some("mango"), device_color: Some("orange") }, NONE).unwrap();
        assert_eq!(street.color, "orange");
    }

    #[test]
    fn has_no_look_for_a_model_it_cannot_draw() {
        assert_eq!(look_of(Reported { device_model: Some("saturn"), device_color: Some("black") }, NONE), None);
        assert_eq!(look_of(Reported::default(), NONE), None);
    }

    #[test]
    fn refuses_a_choice_the_data_has_settled_or_the_variant_does_not_have() {
        let problem = |reported, model, color| look_problem(reported, Chosen { model, color });
        assert_eq!(problem(lite2(Some("blue")), Some("plain"), None), None);
        assert_eq!(problem(lite2(Some("blue")), None, Some("pink")).as_deref(), Some("Цвет колонка сообщила сама."));
        assert_eq!(
            problem(lite2(Some("green")), Some("clock"), None).as_deref(),
            Some("Модель колонка сообщила сама.")
        );
        assert_eq!(
            problem(lite2(None), Some("clock"), Some("coral")).as_deref(),
            Some("Цвет: azure, lilac, pink, graphite.")
        );
        assert_eq!(problem(lite2(None), Some("big"), None).as_deref(), Some("Модель: clock или plain."));
        assert_eq!(
            problem(Reported::default(), Some("clock"), None).as_deref(),
            Some("Вид этой модели колонки пока не выбрать.")
        );
        assert_eq!(problem(Reported::default(), None, None), None);
        // A single-variant model: the model is settled by the firmware, only the colour can be picked.
        let max = Reported { device_model: Some("yandexstation_2"), device_color: None };
        assert_eq!(problem(max, Some("one"), None), None);
        assert_eq!(problem(max, Some("clock"), None).as_deref(), Some("Модель колонка сообщила сама."));
        assert_eq!(problem(max, None, Some("teal")).as_deref(), None);
        assert_eq!(
            problem(max, None, Some("coral")).as_deref(),
            Some("Цвет: graphite, sand, green, red, lilac, teal.")
        );
    }

    #[test]
    fn serializes_as_the_panel_reads_it() {
        let look = serde_json::to_value(look_of(lite2(Some("green")), NONE).unwrap()).unwrap();
        assert_eq!(look["modelFrom"], "station");
        assert_eq!(look["variants"][0]["label"], "Без часов");
        assert_eq!(look["colors"], serde_json::json!(["green"]));
        assert_eq!(look["sticker"], false);
    }

    #[test]
    fn offers_a_sticker_only_to_the_model_sold_with_them() {
        assert_eq!(look_of(lite2(None), NONE).unwrap().sticker, Some(false));
        let mini = Reported { device_model: Some("yandexmini_2"), device_color: None };
        assert_eq!(look_of(mini, NONE).unwrap().sticker, None);
        assert_eq!(serde_json::to_value(look_of(mini, NONE).unwrap()).unwrap()["sticker"], serde_json::Value::Null);
    }
}
