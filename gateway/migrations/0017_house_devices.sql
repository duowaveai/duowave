-- The home's devices at its Турбо (docs/plans/turbo-devices.md): what the centre has seen plugged into each computer
-- that helps it, so that it says «клавиатура у Mac, а он спит» and not «клавиатуры нет», and makes a teacher for a
-- keyboard it saw before. Kept by the centre only; nothing of it travels (src/home/turbo_devices.rs).
--
-- A home has a Турбо per computer (0.18.2), and an entry of the setting `helpers` may pass to another computer — the
-- door `home-computer` taken by the one that joins after the first was removed. So every row names the computer as
-- well, as its contract names itself (`contract.helper.id`, '' while not known): what was seen at one computer is
-- never said of another, and a wake never goes to the one before.
CREATE TABLE house_devices (
    -- The Турбо's id in the setting `helpers`.
    helper TEXT NOT NULL,
    -- The computer at that entry when the thing was seen.
    computer TEXT NOT NULL DEFAULT '',
    -- The kind: `midi`, `screen`, `storage`, …
    kind TEXT NOT NULL,
    -- The thing's id at the Турбо (`in-2`) and its name for people («Keystation 49»).
    device TEXT NOT NULL,
    title TEXT NOT NULL,
    -- Unix ms: first and last seen plugged in.
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    PRIMARY KEY (helper, computer, kind, device)
) WITHOUT ROWID;

-- Where a kind is done when the home has it at several places (owner 20.11, 1, 2): per ear — the device that heard the
-- wish, its room — the place chosen last, and when that was said aloud (said at the first choice and at a change).
-- The ear '' is the house's last use: an ear without a choice of its own leans to it («где пользовались последним»).
CREATE TABLE device_choice (
    kind TEXT NOT NULL,
    -- The device that heard the wish; '' for the house as a whole (its last use, an app's own letter, the panel).
    ear TEXT NOT NULL,
    -- The place: a Турбо's id, or '' for this gateway itself.
    helper TEXT NOT NULL,
    -- The computer at that place when it was chosen ('' for this gateway): a choice of another computer is no choice.
    computer TEXT NOT NULL DEFAULT '',
    chosen_at INTEGER NOT NULL,
    said_at INTEGER,
    PRIMARY KEY (kind, ear)
) WITHOUT ROWID;

-- How a sleeping Турбо is woken over the home network (owner 20.11, 5): the hardware addresses it gave at its last look.
CREATE TABLE turbo_wake (
    helper TEXT PRIMARY KEY NOT NULL,
    -- The computer that gave them: another computer at the entry is not woken by them.
    computer TEXT NOT NULL DEFAULT '',
    -- JSON: ["aa:bb:cc:dd:ee:ff", …].
    macs TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
