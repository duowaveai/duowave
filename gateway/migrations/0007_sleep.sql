-- The sleep (docs/plans/memory.md, «Сон и рефлексия»): the gateway's night work in parts, src/sleep. A night is one
-- run: what started it, when, and its parts' reports. The reflection, its first part, writes the day's summary and
-- the characters' diaries as notes, and marks the conversations it has thought over.

CREATE TABLE sleep_runs (
    id TEXT PRIMARY KEY NOT NULL,
    night TEXT NOT NULL,                -- the day the night started on the station's clock (YYYY-MM-DD)
    started_at TEXT NOT NULL,
    finished_at TEXT,
    trigger TEXT NOT NULL,              -- schedule | manual
    state TEXT NOT NULL,                -- running | done | partial | nothing | failed
    report TEXT NOT NULL DEFAULT '[]',  -- JSON: the parts' reports, in their order
    attempts INTEGER NOT NULL DEFAULT 1,
    rolled_back_at TEXT
);

CREATE INDEX sleep_runs_started ON sleep_runs (started_at);
CREATE INDEX sleep_runs_night ON sleep_runs (night, trigger);

-- What the reflection writes apart from facts: the household's day summary, a character's diary entry; later the
-- weeks and months they fold into.
CREATE TABLE memory_notes (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,                 -- day | diary | week | month
    day TEXT NOT NULL,                  -- the day it is about (the first day of a week or month)
    character_id TEXT,                  -- a diary's character; NULL for the household's summary
    text TEXT NOT NULL,
    lang TEXT NOT NULL,
    source TEXT NOT NULL,               -- reflection | panel
    run_id TEXT,
    visibility TEXT NOT NULL DEFAULT 'household', -- a diary: character, its owner the character
    visibility_owner TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX memory_notes_day ON memory_notes (kind, day);
CREATE INDEX memory_notes_run ON memory_notes (run_id);

-- The night whose reflection thought a conversation over; NULL — still to think over. Rolling the night back
-- clears it, and the conversation waits for the next one.
ALTER TABLE memory_episodes ADD COLUMN reflected_run TEXT;

CREATE INDEX memory_changes_run ON memory_changes (run_id);
