-- The firmware a station named at its last sync (synchronizeState's firmwareVersion): a change of it is an update of
-- the station, said in the journal (station.firmware); the panel says whether the gateway was tried with it.
ALTER TABLE stations ADD COLUMN firmware TEXT;
