-- Packages (src/plugins): plugins, apps, characters and themes live in PACKAGES_DIR (`<id>/<version>/`), what the
-- admin decided lives here. A package without a row is installed and off: nothing of it runs until the admin
-- switches it on.
--
-- `version` is the one that last started well: when a newer package on disk does not start, the gateway keeps
-- running this one, after a restart too.
CREATE TABLE packages (
    id TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    version TEXT,
    updated_at TEXT NOT NULL,
    updated_by TEXT
);

-- A plugin's own key-value store (Host API `kv`): JSON values by key, counted against its storage budget.
CREATE TABLE plugin_kv (
    plugin_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (plugin_id, key)
);
