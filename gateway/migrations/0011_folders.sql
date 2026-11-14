-- The home folder's folders and what a person attached to it (docs/plans/shared-folders.md). A row is the
-- household's word on one folder: who sees it, whether the model may read it and the voice name its files, whether it
-- opens outside the home; for an attached one also where it is on this computer and how it came. A folder without a
-- row keeps the defaults (src/bus/providers/files/rules.rs). Each gateway keeps its own: the paths are this
-- computer's, so the table does not travel with the settings package.
CREATE TABLE folders (
    -- The folder's address: files/home/Фото, files/home/Подключено/Флешка KINGSTON.
    address TEXT PRIMARY KEY NOT NULL,
    -- An attached folder: where it is, the drive's mark to know it again, how it came (drive, dropped, picked).
    path TEXT,
    volume TEXT,
    how TEXT,
    -- A drive known again when it comes back, without asking.
    remember INTEGER NOT NULL DEFAULT 1,
    -- Apps may put and change files in an attached folder (home folders are written by the apps' links).
    write INTEGER NOT NULL DEFAULT 0,
    -- Who sees it, by role: admin,member,guest.
    roles TEXT NOT NULL DEFAULT 'admin,member',
    -- The model may read its names and files though it is in the cloud.
    model INTEGER NOT NULL DEFAULT 0,
    -- The voice may name its files though Yandex says them.
    speak INTEGER NOT NULL DEFAULT 0,
    -- It opens outside the home (through the tunnel).
    remote INTEGER NOT NULL DEFAULT 0,
    added_at TEXT NOT NULL,
    added_by TEXT
);
