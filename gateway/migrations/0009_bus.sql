-- The bus's journal (docs/plans/bus.md): who wrote to whom and how it went — never what the letter carried. Lines
-- are only added; past the place's limit the oldest go (src/bus/journal.rs: 2000 on the station, 20 000 on a
-- computer). Each gateway keeps its own: it does not travel with the settings package.
CREATE TABLE bus_journal (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    trail INTEGER NOT NULL,
    root TEXT NOT NULL,
    kind TEXT NOT NULL,
    from_addr TEXT NOT NULL,
    to_addr TEXT NOT NULL,
    handle TEXT NOT NULL,
    outcome TEXT NOT NULL,
    ms INTEGER NOT NULL DEFAULT 0,
    note TEXT
);

CREATE INDEX bus_journal_from ON bus_journal (from_addr, seq);
CREATE INDEX bus_journal_to ON bus_journal (to_addr, seq);
