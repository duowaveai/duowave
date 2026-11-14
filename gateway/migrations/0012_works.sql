-- The works of the talk (src/talk/works.rs; docs/plans/audio-interface.md, «Запись работы»): one row per work, from
-- what was asked to how it ended — what people asked in a conversation, what they added on the way, what came of it,
-- the question it waits on. A work is found only by its id; who does it (`executor`) keeps its own id of the thing
-- (`target`: the workshop's draft). A work that ran when the gateway stopped is marked `interrupted` at the next
-- start: nothing restarts by itself. Each gateway keeps its own: the table does not travel with the settings package.
CREATE TABLE works (
    id TEXT PRIMARY KEY NOT NULL,
    -- `app` (a widget, a skill, a game, a theme); more kinds come with more executors.
    kind TEXT NOT NULL,
    -- What it becomes: `widget`, `skill`, `game`, `theme`.
    face TEXT,
    executor TEXT NOT NULL,
    target TEXT,
    title TEXT NOT NULL,
    -- `working`, `ready`, `unfinished`, `failed`, `stopped`, `interrupted`, `done`.
    state TEXT NOT NULL,
    -- The whole wish, gathered from the conversation.
    brief TEXT NOT NULL DEFAULT '',
    -- 1: a new version of a thing there is already (an edit, «доделай»), not a new thing.
    again INTEGER NOT NULL DEFAULT 0,
    -- What was added on the way, as JSON: `[{text, at, origin, readAt?}]`.
    notes TEXT NOT NULL DEFAULT '[]',
    -- What came of it, as JSON: `{line, end?}`; `end` — `desk`, `fitting`, `parked`, `declined`.
    result TEXT,
    -- The question it waits on, as JSON: `{text, until}`.
    question TEXT,
    -- Where it was asked, as JSON: the activator, its name, the character, who, the device, by voice or in text.
    origin TEXT NOT NULL,
    -- The device of `origin`, for the lists.
    device TEXT NOT NULL,
    -- When its result was said to the end; empty — not said yet.
    said_at INTEGER,
    -- The model's spending on it.
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write INTEGER NOT NULL DEFAULT 0,
    rounds INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    ended_at INTEGER
);

CREATE INDEX works_device ON works (device, started_at);
CREATE INDEX works_target ON works (executor, target);

-- Where a request came from (the activator, the character, who, the device, by voice or in text) and what the model
-- spent on it (in, out, from the cache, into the cache, rounds, the model), both as JSON.
ALTER TABLE requests ADD COLUMN origin TEXT;
ALTER TABLE requests ADD COLUMN tokens TEXT;
