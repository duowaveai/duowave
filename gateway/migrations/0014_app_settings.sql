-- The gateway's settings with an app's own value on top (docs/plans/own-server.md, «Общие настройки приложений»):
-- the keys of `settings`, one value per app, JSON as there. Not a table of one feature: «Выход в интернет» keeps its
-- per-row rules here (`egress.route`, `egress.on_down`, `egress.learned`), tomorrow another feature keeps its own.
-- `app_id` — a package's id, or a reserved id of who is not an app: `system:alice`, `provider:openai`, `mcp:exa`.
-- No foreign key to `packages`: an app removed keeps its rows — installed again, it finds its rules; they go when its
-- data goes for good (plugins/history/gc.rs).
CREATE TABLE app_settings (
    app_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (app_id, key)
);
