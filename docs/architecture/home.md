# Home and devices (`src/home/`, plan)

Part of DuoWave's description — [ARCHITECTURE.md](../ARCHITECTURE.md). Paths `src/…` are from `gateway/`.

One home, one centre. The centre is the gateway on a device that does not sleep (a station, a box; a computer only
while it is alone in the home): it decides who answers, holds the conversation and is the only one that writes the
home — people, characters, "Brain", keys, connections, rooms, apps, memory. The other devices are ears (they hear for
the centre) and Turbos (power, `src/helpers`). People see the roles as the labels "Hub" and "Turbo" and the "hears"
mark; the word "ear" is for diagrams and code only.

- `devices.rs` — `GET /api/v1/home/devices`: the centre, ears, Turbos with roles and state, the event
  `home.devices.changed`. A computer Turbo and an ear ask the centre (pair key): they have no home of their own. The
  panel's home line, "Settings → Home → Devices" and the desk scene read it.
- `copy.rs`, `queue.rs` — the home copy: a device that can answer by itself (a computer Turbo, an ear) takes the
  centre's new revision and applies it without a restart (`transfer::import_keeping`, its own access keys stay); memory
  adds up both ways by record; changes made without the centre wait in `home_queue` (migration 16) and go in order.
- `ear.rs` — a second station as an ear: its voice socket goes on to the centre (`bridge`); without the centre it is
  served locally from the copy; a change of the home on an ear is refused with words (`guard`). "Make the hub" — the
  centre moves over the home network, without SSH; a computer Turbo follows it.
- `rooms.rs` — rooms differ only in Alice's mode, the volume and "Who answers here" (`device_rooms`).
- `character.rs`, `telegram.rs` — the character with a voice and Telegram belong to the home: the centre names the
  Turbo that voices it and keeps the bot (`/home/v1/character`, `/home/v1/messenger/bot`, Turbo contract version 2).
- `apps.rs` — one "Installed" list by place; an app for a computer is installed on a station centre as a face
  (`Manifest::face_at`: phrases, window, widget), and its code on the Turbo over the link (`/home/v1/plugins/install`).

## A home of stations without a Turbo: how it works

People see the centre as "Hub"; in code and in this document it is the centre (`centre`). In the example below the hub
is Kitchen and the ear is Bedroom.

**Who holds what.** Only the hub writes the home: people, characters, "Brain" and the AI keys, connections, rooms,
apps, memory. An ear has no home of its own — only a copy of the home. Rooms differ only in the station's name and
room, Alice's mode, the volume and "Who answers here" (`rooms.rs`); everything else is shared.

**How a station becomes an ear** (stage 5 of `home-setup-flow.md`). A station with its own DuoWave says a code, which is
typed into the home's panel. After that:
- its settings, people and characters are replaced by the home's, its memory merges with the home's;
- it keeps the hub's pair key (`earof:key`) and looks at the hub every 30 s (`LOOK`), taking a new revision of the
  home;
- its open voice socket closes: the station reconnects, and its voice goes through the hub at once;
- the hub lets its voice in without "Allow": the ear opens the hub's socket with the hub's pair key
  (`x-duowave-ear-key`), which the hub takes off before anything goes on to Yandex.

**Voice.** You speak in the bedroom. DuoWave on Bedroom forwards its station's voice socket to the hub (`ear::bridge`),
the station's headers go with it. The hub decides who answers by the mode and character of the Bedroom room. In the
"Alice" mode the hub opens Yandex on Bedroom's behalf — its Alice answers. The answer sounds in the bedroom.

**The hub is gone** (off, no Wi-Fi). The ear does not wait: if the hub did not take the socket within 2 s (`DIAL`), the
ear serves the voice itself from its last copy of the home. The home cannot be changed on an ear, hub or no hub: the
change is refused with words ("the home is changed in the hub's panel", `ear::guard`) — made here, it would be lost at
the next copy. The queue of changes made without the centre (`queue.rs`) belongs to a computer Turbo, not to an ear.
Memory adds up both ways by record.

**Without a Turbo** there are no computer "muscles": the character's voice and recognition come from the cloud or from
Alice, the AI through a cloud key set on the hub (none chosen — the AI does not answer). "Home status" says so.
Everything else the hub does itself, on the station (the Lite 2 has 123 MB of RAM, see its limits).

**A computer in such a home** (if there is one) keeps a link with each station by its pair key. Its record of an ear
stays the ear's record while it follows the hub (`placement/watch.rs`, `follow_centre`); it takes each station's id
from its `/health` — the panel tells the home's stations from those "not in the home" by it. A station installed from
the terminal (not by a move) knows itself by its voice — the station that dials from this same machine
(`placement::service::station_self`).

**What is checked.**
- Gateway tests: two gateways in one process (`placement/routes.rs`): joining by code, an ear by code, the hub letting
  in its ear's voice, a stranger station waiting for approval.
- 23.11 on pretend stations (`scripts/pretend/`, `docs/plans/pretend-stations.md`): three stations and a Turbo on one
  Mac — search, the Turbo joining by the hub's code, an ear by its own code, the ear's voice through the hub, "Devices"
  and "Home status" in the panel.
- Not checked: on two real stations; an ear answering by itself without the hub; sound.
