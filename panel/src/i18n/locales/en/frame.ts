// Texts: frame/* — an app's window and its bridge.

import type { Translation } from '../../types';
import type ru from '../ru/frame';

const en: Translation<typeof ru> = {
    appCrashedLog: 'Window: the app crashed',
    appCrashed: 'The app’s window crashed:',
    moduleNotHere: 'the module is not from this gateway',
    noPlugin: 'the module exports no plugin: export default definePlugin({…})',
    noPart: (part: string) => `the app has no “${part}”`,
    failed: 'it did not work',
    noTask: 'no task',
    midiTooBig: 'The file is over 256 KB — not a simple melody.',
    probeSilent: (id: string) => `the window of the app ${id} did not answer`,
    frameTitle: (id: string) => `Window of the app ${id}`,
    reloadedItself: 'the window reloaded itself — closed',
    notAFile: 'not a file',
    asks: (id: string, title: string) => `“${id}” asks: ${title}`,
    didNotOpen: 'The window did not open:',
};

export default en;
