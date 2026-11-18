// Texts: screens/first-run.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/firstRun';

const en: Translation<typeof ru> = {
    title: (name: string) => `DuoWave is already running on the speaker “${name}”`,
    about: 'The speaker answers on its own, even when the computer sleeps. This computer can help it while it is on: recognise speech, speak the answers, take heavy tasks, keep settings backups. You can move DuoWave to the computer later: “Settings” → “Speaker”.',
    help: 'Computer as the speaker’s Turbo',
    moreStations: (names: string) => `More speakers with DuoWave: ${names}.`,
};

export default en;
