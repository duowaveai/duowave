// Texts: sdk/*.
import type { Translation } from '../../types';
import type ru from '../ru/sdk';

const NOTES = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];

const en: Translation<typeof ru> = {
    // notes.ts: scientific pitch names — C4 is middle C.
    noteShort: (index: number) => NOTES[index],
    noteFull: (index: number, octave: number) => `${NOTES[index]}${octave}`,

    // piano-keys.tsx
    keyboard: 'Keyboard',

    // midi.ts
    noBrowserMidi: 'This window has no browser MIDI (the DuoWave app, Safari): the gateway on the computer listens to the keyboard.',
    httpNoMidi: 'The browser does not give MIDI to a page at the home address (http): open the panel on the computer with the keyboard.',
    exampleKeyboard: 'Example: an on-screen keyboard. Your own keyboard connects after “yes”.',
    gatewayNoKeyboardBrowser: 'The gateway has no keyboard. Is the keyboard on this device? Connect it through the browser.',
    gatewayNoKeyboard: 'The gateway has no keyboard.',
    plugKeyboardBrowser: 'Plug the keyboard into the computer running DuoWave (USB). Is the keyboard on this device? Connect it through the browser.',
    plugKeyboard: 'Plug the keyboard into the computer running DuoWave (USB).',
    midiDenied: 'The browser did not allow MIDI: the permission is in the site settings (the lock icon in the address bar).',
    browserNoKeyboard: 'The browser does not see a keyboard: plug it into this device.',
    midiTooBig: 'The file is over 256 KB — this is not a simple melody.',

    // index.tsx
    fnFailed: (fn: string) => `${fn} did not run`,
    appOffOnStation: (title: string) => `The app “${title}” is turned off on the speaker.`,
};

export default en;
