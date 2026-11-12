-- Memory, stage 1 «Основа» (docs/plans/memory.md): the household graph — entities with other names and facts with
-- dates — the episodes of the day and the memory's own log of changes for «Отменить».
--
-- Every record keeps who said it, where, who could hear it, who sees it and whose it is (the plan's «Люди дома и
-- приватность»), so the memory can split by the people of the house later without a new schema. Times are RFC 3339
-- text in UTC.

CREATE TABLE memory_entities (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,                 -- household | person | pet | room | thing | place | org | character
    name TEXT NOT NULL,
    -- A person of the house can point at their panel sign-in and their Telegram contact (later, with the split).
    account_id TEXT,
    contact TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX memory_entities_kind ON memory_entities (kind);

-- Other names: «Мария», «сестра Пети». `norm` is the stemmed lower-case form used to find the entity in text.
CREATE TABLE memory_aliases (
    entity_id TEXT NOT NULL REFERENCES memory_entities (id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    norm TEXT NOT NULL,
    PRIMARY KEY (entity_id, norm)
);

CREATE INDEX memory_aliases_norm ON memory_aliases (norm);

CREATE TABLE memory_facts (
    id TEXT PRIMARY KEY NOT NULL,
    subject_id TEXT NOT NULL REFERENCES memory_entities (id) ON DELETE CASCADE,
    relation TEXT,                      -- «аллергия», «работает в», «сестра»; NULL when said as free text
    object_id TEXT REFERENCES memory_entities (id) ON DELETE SET NULL,
    value TEXT,
    text TEXT NOT NULL,                 -- the fact as a sentence, for the prompt and the panel
    lang TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'fact',  -- fact | opinion | insight | lesson | todo
    viewpoint TEXT,                     -- the character whose opinion it is; NULL for a fact
    valid_from TEXT,
    valid_to TEXT,                      -- set when closed: «больше не так», «забудь», replaced
    closed_reason TEXT,
    replaced_by TEXT,
    confidence REAL NOT NULL DEFAULT 1.0,
    importance INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,  -- always in the prompt
    mentions INTEGER NOT NULL DEFAULT 0,
    last_prompted_at TEXT,
    rejected INTEGER NOT NULL DEFAULT 0, -- a person turned this insight down: never infer it again
    source TEXT NOT NULL,               -- voice | model | panel | extract | reflection | package
    episode_id TEXT,
    -- Who said it and who could hear it.
    said_by_kind TEXT NOT NULL,         -- user | household | apiKey | system | contact
    said_by_id TEXT,
    channel TEXT NOT NULL,              -- station | panel | telegram | api | system
    station TEXT,
    audience TEXT NOT NULL,             -- speaker | room
    heard_by TEXT,                      -- the character who heard it
    -- Who sees it and whose it is.
    visibility TEXT NOT NULL DEFAULT 'household', -- household | person | people | character | world
    visibility_owner TEXT,
    owner_id TEXT,                      -- NULL: the household, that is the admin
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX memory_facts_subject ON memory_facts (subject_id, valid_to);
CREATE INDEX memory_facts_object ON memory_facts (object_id);
CREATE INDEX memory_facts_created ON memory_facts (created_at);

-- Who sees a record shared with a list of people (visibility 'people').
CREATE TABLE memory_visible_to (
    record_id TEXT NOT NULL,
    person_id TEXT NOT NULL REFERENCES memory_entities (id) ON DELETE CASCADE,
    PRIMARY KEY (record_id, person_id)
);

-- One question and answer: kept word for word for 7 days, then only what was learned from it stays.
CREATE TABLE memory_episodes (
    id TEXT PRIMARY KEY NOT NULL,
    at TEXT NOT NULL,
    station TEXT,
    character_id TEXT,
    question TEXT NOT NULL,
    answer TEXT NOT NULL,
    said_by_kind TEXT NOT NULL,
    said_by_id TEXT,
    channel TEXT NOT NULL,
    audience TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'household',
    visibility_owner TEXT,
    extracted INTEGER NOT NULL DEFAULT 0 -- the разбор has read it (stage 2)
);

CREATE INDEX memory_episodes_at ON memory_episodes (at);

-- The memory's own log: what changed, by whom and why, with the state before and after, for «Отменить» and the
-- night report. People's changes also go to the gateway's audit log («кто что менял»).
CREATE TABLE memory_changes (
    id TEXT PRIMARY KEY NOT NULL,
    at TEXT NOT NULL,
    actor_kind TEXT NOT NULL,           -- user | household | apiKey | system
    actor_id TEXT,
    action TEXT NOT NULL,               -- add | edit | close | reopen | erase | pin | unpin | alias | merge | rename
    target TEXT NOT NULL,               -- fact | entity
    target_id TEXT NOT NULL,
    summary TEXT NOT NULL,              -- the record's text, for the list
    before TEXT,                        -- JSON
    after TEXT,                         -- JSON
    reason TEXT,
    run_id TEXT,                        -- the night that made it (stage 6)
    undone_at TEXT
);

CREATE INDEX memory_changes_at ON memory_changes (at);
CREATE INDEX memory_changes_target ON memory_changes (target_id);
