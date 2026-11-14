-- Long tasks of apps (src/tasks; docs/plans/feed.md, stage 1): what an app started and reports on — whose it is, what
-- it is in words for people, how far it got, how it ended. A task that ran when the gateway stopped is marked
-- `interrupted` at the next start: nothing restores it. Ended tasks are kept a day (src/tasks: RESULT_KEPT_MS), a few
-- per app. Each gateway keeps its own: the table is not among those that travel with the settings package
-- (src/placement/transfer.rs), so a move to another computer leaves the tasks behind.
CREATE TABLE app_tasks (
    id TEXT PRIMARY KEY NOT NULL,
    app TEXT NOT NULL,
    title TEXT NOT NULL,
    -- `running`, `done`, `failed`, `cancelled`, `interrupted`.
    state TEXT NOT NULL,
    percent INTEGER,
    text TEXT NOT NULL DEFAULT '',
    -- The result as JSON (at most 64 KB), of a task that is done.
    result TEXT,
    error TEXT,
    -- Its steps as JSON: `[{at, percent?, text}]`, the last hundred.
    log TEXT NOT NULL DEFAULT '[]',
    started_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    ended_ms INTEGER
);

CREATE INDEX app_tasks_app ON app_tasks (app, started_ms);
