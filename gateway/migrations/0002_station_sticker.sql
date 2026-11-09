-- Whether a station wears a sticker (the Lite 2 comes with cats in the box): a person's choice, the panel draws it.
-- NULL — not said, drawn without one; only models sold with stickers offer the choice (src/stations/looks.rs).
ALTER TABLE stations ADD COLUMN sticker INTEGER;
