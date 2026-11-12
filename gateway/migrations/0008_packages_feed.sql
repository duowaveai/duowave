-- Own sources of packages and the feed's `hidden` (docs/plans/custom-feeds.md). `feed`: the address of the feed a
-- package was installed from (NULL: by hand, or from the catalog before 0.6.0 — the main feed then). `hidden`: that
-- feed says to hide it (or the own source it came from is closed): not listed, its code does not run, its characters
-- are not offered; its files stay, and it comes back when the feed stops saying so.
ALTER TABLE packages ADD COLUMN feed TEXT;
ALTER TABLE packages ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
