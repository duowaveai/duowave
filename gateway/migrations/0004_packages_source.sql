-- Packages from the catalog (docs/plans/characters.md): where a package came from and whether it is one we checked.
-- `source`: catalog (installed from the feed by a button) or local (copied in by hand, or installed before the
-- catalog); `verified`: the catalog's own mark, 0 for everything else.
ALTER TABLE packages ADD COLUMN source TEXT NOT NULL DEFAULT 'local';
ALTER TABLE packages ADD COLUMN verified INTEGER NOT NULL DEFAULT 0;

-- Characters of the packages (src/characters) beside the built-in cards: `<package>:<character>` ids, so the choice
-- by id keeps working. `text` is the prompt with the default tones; `tones` the regulators as JSON; `avatar` a path
-- in the package's assets/. The built-in cards keep NULL here.
ALTER TABLE characters ADD COLUMN package TEXT;
ALTER TABLE characters ADD COLUMN version TEXT;
ALTER TABLE characters ADD COLUMN color TEXT;
ALTER TABLE characters ADD COLUMN tones TEXT;
ALTER TABLE characters ADD COLUMN avatar TEXT;
