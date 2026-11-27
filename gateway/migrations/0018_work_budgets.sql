-- Router stage 2, the budgets (docs/plans/router-and-works.md, block A, 5).
--
-- A single work's own budget (docs/plans/router-and-works.md, block A, 5; src/router/budgets.rs): it holds over its
-- folder's and the general one. By the work's id in the talk's records and when that work began — ids start over once
-- the old works are gone, so a budget holds only for the work it was given to. `budget` is JSON: `{measure, limit,
-- steps}`. Kept as long as the deals (90 days), then cleaned. Each gateway keeps its own: nothing of it travels.
CREATE TABLE work_budgets (
    work TEXT PRIMARY KEY NOT NULL,
    since INTEGER NOT NULL,
    budget TEXT NOT NULL,
    at INTEGER NOT NULL
);

-- The model a work's agent session was opened with (src/works/runner.rs): a step of the budget may put the next run
-- on a cheaper model, and another model is another session — «продолжай» goes on in the session only while the model
-- is the same. Empty — the agent's own choice; NULL — not known (a work of an older gateway: its `model`).
ALTER TABLE folder_works ADD COLUMN session_model TEXT;
