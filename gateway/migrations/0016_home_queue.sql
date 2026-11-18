-- The home copy's queue (docs/plans/home-and-devices.md, stage 3): a change made on a device that holds a copy of the
-- home while its centre is away — a person's edit in the home's panel on the computer — waits here and goes to the
-- centre in order when it is back (src/home/queue.rs). In the settings the last change wins; the centre's history
-- keeps the one before. Only a copy holder writes here; nothing of it travels.
CREATE TABLE home_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    -- The request as the panel made it: its method, its path at the centre (`/api/v1/…` with its query) and body.
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    content_type TEXT,
    body BLOB,
    -- Who made it, as the pass names people to the centre (src/placement/proxy.rs).
    person TEXT NOT NULL DEFAULT '',
    person_id TEXT,
    role TEXT NOT NULL,
    -- Tries so far and why the last one did not go; a change the centre refused is kept with its reason, not sent again.
    tries INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    refused INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX home_queue_waiting ON home_queue (refused, id);
