-- The gateway's tables, as the Node gateway's schema had them at its last migration (drizzle 0009), in one step: the
-- Rust gateway starts its own database. Every later change is a new file here (NNNN_name.sql), never an edit of
-- this one: the gateway refuses to start when an applied migration's text changed.
--
-- JSON columns hold text; booleans are 0/1; times are ISO 8601 in UTC (2026-11-06T02:11:00.331Z).

-- What must outlive a restart, a row per kind and device, the records as JSON (our timers, alarms, reminders, the
-- player's last state): each device's list is written whole.
CREATE TABLE state (
    kind TEXT NOT NULL,
    device TEXT NOT NULL,
    items TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (kind, device)
);

-- Named settings (the routes, the mode, the brain): a JSON value per key, `is_default` marking the value the gateway
-- seeded and nobody has changed.
CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);

-- The household's people (src/access). The first to register becomes the admin. A person without a password is a
-- profile only — a child the station will recognise by voice or by name, who never signs in to the panel. The role
-- is admin, member or guest; `restrictions` are the admin's per-person allow/deny grants on top of the role;
-- bumping `token_version` signs the person out everywhere.
CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    password_hash TEXT,
    role TEXT NOT NULL DEFAULT 'member',
    status TEXT NOT NULL DEFAULT 'active',
    token_version INTEGER NOT NULL DEFAULT 0,
    restrictions TEXT NOT NULL DEFAULT '{}',
    profile TEXT NOT NULL DEFAULT '{}',
    last_login_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX users_name_unique ON users (name);

-- A person's own settings over the global ones (their voice, their Alisa): a JSON value per key.
CREATE TABLE user_settings (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, key)
);

-- Keys for scripts and agents (OpenClaw, Hermes, curl): exactly the grants the admin gave, the secret shown once and
-- kept only as its SHA-256.
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    secret_hash TEXT NOT NULL,
    grants TEXT NOT NULL,
    created_by TEXT,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    expires_at TEXT,
    revoked_at TEXT
);

-- Who changed what: the admin's actions and the sign-ins, from the gateway's events.
CREATE TABLE audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    at TEXT NOT NULL,
    action TEXT NOT NULL,
    actor_kind TEXT,
    actor_id TEXT,
    actor_name TEXT,
    details TEXT NOT NULL DEFAULT '{}'
);

-- The voice catalog, as data: what is offered, not what is installed (the files on disk are the reality). A voice
-- model is what downloads — one file; the voices inside it are what speaks. `source`: builtin (the showcase) or
-- registry (the open HuggingFace voices).
CREATE TABLE voice_models (
    id TEXT PRIMARY KEY NOT NULL,
    engine TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    badge TEXT,
    license TEXT,
    locale TEXT,
    quality TEXT,
    size TEXT,
    url TEXT,
    file TEXT,
    source TEXT NOT NULL DEFAULT 'builtin',
    updated_at TEXT NOT NULL
);

-- A voice inside a model — what the synthesis request asks for (`name`: baya, en_GB-cori-high).
CREATE TABLE voices (
    id TEXT PRIMARY KEY NOT NULL,
    model_id TEXT NOT NULL REFERENCES voice_models (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    short TEXT,
    updated_at TEXT NOT NULL
);

-- The recognition catalog: the showcase cards; `model` is what the recognition server loads.
CREATE TABLE asr_models (
    id TEXT PRIMARY KEY NOT NULL,
    model TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    badge TEXT,
    license TEXT,
    source TEXT,
    updated_at TEXT NOT NULL
);

-- The character cards: who Alisa is. The built-in ones seed on first start; the panel's edits pick by id.
CREATE TABLE characters (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    badge TEXT,
    text TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- The stations the gateway serves (src/stations): approved before they are answered — the first one is trusted on
-- first use, later ones wait for the admin. `device` is the station's tag as the journal names it; `last_address`
-- binds the station's own HTTP (the call client) to the address it connects from. `device_model`, `device_color`:
-- what it says of itself at sync; `model`, `color`: its look as a person chose it (empty — the default).
CREATE TABLE stations (
    device TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    last_address TEXT,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    device_model TEXT,
    device_color TEXT,
    model TEXT,
    color TEXT
);

-- The journal of requests (src/requests): what was asked, what was answered, who did each step and how long it
-- took. A row is written when a request ends; `requests_keep_days` prunes old rows. Only who may read the journal
-- (journal.view) gets the words.
CREATE TABLE requests (
    id TEXT PRIMARY KEY NOT NULL,
    device TEXT NOT NULL,
    session TEXT,
    kind TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ms INTEGER,
    text TEXT,
    answer TEXT,
    capability TEXT,
    executor TEXT,
    steps TEXT NOT NULL DEFAULT '[]',
    error TEXT,
    parent TEXT
);
CREATE INDEX requests_started_at ON requests (started_at);
CREATE INDEX requests_device ON requests (device, started_at);

-- The gateway's secrets (src/secrets): model keys, MCP tokens and variables, OAuth tokens, the panel's signing
-- secret. Sealed with AES-256-GCM under a key kept in its own file beside the database, each bound to its id; the
-- database and its copies hold only ciphertext. `id` is a path: llm:<provider>:key, mcp:<server>:env:NAME.
CREATE TABLE secrets (
    id TEXT PRIMARY KEY NOT NULL,
    ciphertext TEXT NOT NULL,
    iv TEXT NOT NULL,
    tag TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Model providers added in the panel (src/dialog/brain): a preset or a custom address. The key is in `secrets` as
-- llm:<id>:key; which provider and model answer is the `brain` setting. `options`: the preset's fields beyond the
-- key (never secrets); `models`: what the service listed at the last check; `last_check`: its result.
CREATE TABLE llm_providers (
    id TEXT PRIMARY KEY NOT NULL,
    preset TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    base_url TEXT,
    options TEXT NOT NULL DEFAULT '{}',
    models TEXT NOT NULL DEFAULT '[]',
    last_check TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- MCP servers added in the panel (src/mcp): a preset or one's own program or address. Their tools reach the model
-- only when on and approved as they are; the server's instructions only as the owner approved them. `config` holds
-- how to reach it (values of variables, headers and tokens are in `secrets`, only their names here).
CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY NOT NULL,
    preset TEXT,
    title TEXT NOT NULL,
    transport TEXT NOT NULL,
    config TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    note TEXT NOT NULL DEFAULT '',
    instructions TEXT,
    tools TEXT NOT NULL DEFAULT '{}',
    capability TEXT,
    voice INTEGER NOT NULL DEFAULT 1,
    admin_only INTEGER NOT NULL DEFAULT 0,
    outside INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
