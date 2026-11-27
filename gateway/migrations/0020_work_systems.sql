-- Work systems, stage 1 (docs/plans/work-systems.md): which system leads a work, and whether it was begun outside
-- DuoWave — in the system itself, by hand (a Claude Code session in a terminal, a bb thread). Empty `system`: the
-- executor's own, our runner for the works in folders. Each gateway keeps its own: nothing of it travels.
ALTER TABLE works ADD COLUMN system TEXT;
ALTER TABLE works ADD COLUMN outside INTEGER NOT NULL DEFAULT 0;
-- A work begun outside is quiet (D1): its end is not said unasked. «Следи за этой» makes it said, «не следи» quiet
-- again.
ALTER TABLE works ADD COLUMN watched INTEGER NOT NULL DEFAULT 0;
