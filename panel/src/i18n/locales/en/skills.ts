// Texts: screens/skills.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/skills';

const en: Translation<typeof ru> = {
    back: 'Apps',
    workshop: 'Workshop',
    contributedFailed: (title: string, plugin: string) =>
        `“${title}” did not open: the “${plugin}” app has an error (details in the browser console). Other apps work.`,
    placeMemory: 'Memory',
    placeMemorySub: 'what characters remember about the home: people, links, pause',
    placeInstalled: 'Installed apps',
    placeInstalledSub: 'turn on, turn off, configure, update',
    placeCatalog: 'Add from the catalog',
    placeCatalogSub: 'apps, characters and themes from DuoWave and the community',
    placeWorkshop: 'Make your own app',
    placeWorkshopSub: 'the workshop agent will write it from your description',
    placeFromApp: 'from the app',
    listLabel: 'Memory, installed and new',
    speakerCan: 'What the speaker can do',
    unsupportedTitle: 'Not supported yet',
    unsupportedNone: 'The speaker can do everything it was asked.',
    teach: 'Teach',
};

export default en;
