// Texts: screens/first-run.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/firstRun';

const en: Translation<typeof ru> = {
    title: (name: string) => `DuoWave is already running on the speaker “${name}”`,
    about: 'The speaker answers on its own, even when the computer sleeps. This computer will become its helper: hearing, voice, model, settings backups and the skill workshop. You can move DuoWave to the computer later: “Settings” → “Speaker”.',
    help: 'Help the speaker',
    moreStations: (names: string) => `More speakers with a gateway: ${names}.`,
};

export default en;
