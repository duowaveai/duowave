//! The memory's records and their labels (docs/plans/memory.md, «Слои», «Граф дома и миров», «Люди дома и
//! приватность»). Every label is stored as text in SQLite; these types are its only spelling.

use serde::{Deserialize, Serialize};

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

text_enum!(
    /// What a node of the graph is. The household itself is one entity: habits and household events are its facts.
    EntityKind {
        Household => "household",
        Person => "person",
        Pet => "pet",
        Room => "room",
        Thing => "thing",
        Place => "place",
        Org => "org",
        Character => "character",
    }
);

text_enum!(
    /// The kind of a record: a plain fact, a character's opinion, and what reflection adds later.
    FactKind {
        Fact => "fact",
        Opinion => "opinion",
        Insight => "insight",
        Lesson => "lesson",
        Todo => "todo",
    }
);

text_enum!(
    /// How a record came in.
    Source {
        /// An explicit «запомни» by voice, without the model.
        Voice => "voice",
        /// The model's `remember` tool, asked or in passing.
        Model => "model",
        /// A person in the panel.
        Panel => "panel",
        /// The разбор after a conversation (stage 2).
        Extract => "extract",
        /// Reflection during sleep (stage 6).
        Reflection => "reflection",
        /// A character's package (stage 3).
        Package => "package",
    }
);

text_enum!(
    /// Where it was said.
    Channel {
        Station => "station",
        Panel => "panel",
        Telegram => "telegram",
        Api => "api",
        System => "system",
    }
);

text_enum!(
    /// Who could hear it: only the one who said it (their own sign-in, a Telegram chat), or anyone in the room.
    Audience {
        Speaker => "speaker",
        Room => "room",
    }
);

text_enum!(
    /// Who sees a record. Today everything grown from talk is `Household`; the rest waits for the people of the house.
    Visibility {
        Household => "household",
        /// One person: `visibility_owner` is their entity id.
        Person => "person",
        /// A list of people in `memory_visible_to`.
        People => "people",
        /// A character's personal layer: `visibility_owner` is the character id.
        Character => "character",
        /// A world's canon: `visibility_owner` is the world id.
        World => "world",
    }
);

text_enum!(
    /// Who said it: the kind of the gateway's `Principal`, or a Telegram contact.
    SpeakerKind {
        User => "user",
        Household => "household",
        ApiKey => "apiKey",
        System => "system",
        Contact => "contact",
    }
);

/// Who said a record and where: kept from stage 1 so the memory can later split by the people of the house.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Origin {
    pub said_by_kind: SpeakerKind,
    /// A user's id, a contact's name; `household` for a voice at a station.
    pub said_by_id: Option<String>,
    pub channel: Channel,
    /// The station's id, when said at a station.
    pub station: Option<String>,
    pub audience: Audience,
    /// The character who heard it.
    pub heard_by: Option<String>,
}

impl Origin {
    /// A voice at a station: the person is not known, anyone in the room could hear.
    pub fn station_voice(station: Option<&str>, character: Option<&str>) -> Origin {
        Origin {
            said_by_kind: SpeakerKind::Household,
            said_by_id: Some("household".into()),
            channel: Channel::Station,
            station: station.map(str::to_string),
            audience: Audience::Room,
            heard_by: character.map(str::to_string),
        }
    }

    /// A person signed in to the panel: only they could hear what they typed or said there.
    pub fn panel_user(user_id: &str) -> Origin {
        Origin {
            said_by_kind: SpeakerKind::User,
            said_by_id: Some(user_id.to_string()),
            channel: Channel::Panel,
            station: None,
            audience: Audience::Speaker,
            heard_by: None,
        }
    }

    /// The gateway itself (reflection, a package, a scenario).
    pub fn system() -> Origin {
        Origin {
            said_by_kind: SpeakerKind::System,
            said_by_id: None,
            channel: Channel::System,
            station: None,
            audience: Audience::Speaker,
            heard_by: None,
        }
    }
}

/// Who is asking: what they may see decides what goes into the prompt, the search and the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewer {
    /// The person speaking, when known (their entity id). A voice at a station is not known yet.
    pub person: Option<String>,
    /// The character answering.
    pub character: Option<String>,
    /// The character's world.
    pub world: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spells_every_label_one_way() {
        for kind in [EntityKind::Household, EntityKind::Person, EntityKind::Pet, EntityKind::Character] {
            assert_eq!(EntityKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Visibility::parse("people"), Some(Visibility::People));
        assert_eq!(SpeakerKind::parse("apiKey"), Some(SpeakerKind::ApiKey));
        assert_eq!(Source::parse("nope"), None);
        assert_eq!(serde_json::to_string(&Audience::Room).unwrap(), "\"room\"");
    }

    #[test]
    fn knows_who_could_hear() {
        assert_eq!(Origin::station_voice(Some("kitchen"), Some("alice")).audience, Audience::Room);
        assert_eq!(Origin::panel_user("u1").audience, Audience::Speaker);
    }
}
