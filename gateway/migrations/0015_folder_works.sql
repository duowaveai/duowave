-- The works in folders (src/works; docs/plans/router-and-works.md, block A, 9): how the helper leads a work in a
-- folder the owner added in the panel — which folder and agent, its own branch and work copy, the agent's session,
-- how it pays, where it is, what the folder's checks said. The talk's record of the same work is in `works`; its
-- `target` is this row's `id`. Only a computer's gateway writes here; each keeps its own: neither table travels with
-- the settings package. Never the content of a file: the agent's journal is a file in the data folder.
CREATE TABLE folder_works (
    id TEXT PRIMARY KEY NOT NULL,
    -- The folder's id in the setting `work_folders`, and its name when the work began.
    folder TEXT NOT NULL,
    folder_name TEXT NOT NULL DEFAULT '',
    -- `claude-code`, or an agent card's id.
    agent TEXT NOT NULL,
    -- The model asked for; empty — the agent's own choice.
    model TEXT,
    title TEXT NOT NULL DEFAULT '',
    -- What was asked: the task as the person gave it.
    task TEXT NOT NULL DEFAULT '',
    -- The work's branch (`duowave/<name>`) and what it began from: the folder's branch then, and its commit.
    branch TEXT NOT NULL,
    base TEXT,
    base_commit TEXT,
    -- The work copy (a `git clone --local` in the data folder); empty once it was cleaned away.
    copy TEXT,
    -- How the agent pays: `plan` (a subscription's sign-in), `key` (a key the gateway keeps), `agent` (the agent's own).
    billing TEXT,
    -- The agent's session: the runs after the first go on in it.
    session TEXT,
    -- `queued`, `working`, `waiting` (a plan's limit), `ready`, `unfinished`, `failed`, `stopped`, `interrupted`.
    state TEXT NOT NULL,
    -- Why it ended so, in a word: `limit`, `auth`, `billing`, `budget`, `refused`, `no-agent`, `git`, `failed`.
    why TEXT,
    -- Until when a plan's limit holds it (ms since the epoch).
    wait_until INTEGER,
    -- The agent's last message, and the folder's checks as JSON: `[{command, ok, code, ms, tail}]`; `skipped` when
    -- the folder has checks and no preparation.
    last TEXT,
    checks TEXT,
    -- Committed by the work itself (the agent left changes), and whether the branch reached the folder.
    committed INTEGER NOT NULL DEFAULT 0,
    fetched INTEGER NOT NULL DEFAULT 0,
    -- What its runs spent as the agent reported it; empty — nobody said a price.
    usd_micro INTEGER,
    runs INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    ended_at INTEGER
);

CREATE INDEX folder_works_folder ON folder_works (folder, created_at);

-- The runs of a work: each call of the agent — the work itself, a note added on the way, a continuation. Its journal is
-- `works/<work>/runs/<n>.jsonl` in the data folder (the agent's events as they came) beside `<n>.steps.jsonl`.
CREATE TABLE work_runs (
    work TEXT NOT NULL,
    n INTEGER NOT NULL,
    -- `work`, `note`, `resume`.
    what TEXT NOT NULL,
    model TEXT,
    session TEXT,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    -- `ok`, `failed`, `limit`, `budget`, `stopped`, `interrupted`; empty — going.
    outcome TEXT,
    -- The router's deal of the run (`model_deals.id`).
    deal INTEGER,
    -- The journal was cut at its limit: only the end was written after it.
    cut INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (work, n)
) WITHOUT ROWID;
