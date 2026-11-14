// Texts: screens/say.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/say';

const en: Translation<typeof ru> = {
    title: 'Say through the speaker',
    stationOnline: 'speaker online',
    stationOffline: 'speaker offline',
    placeholder: 'What should it say?',
    inputLabel: 'Text for the speaker',
    sendLabel: 'Say',
    said: 'The speaker said it.',
    noAnswer: 'The speaker did not respond — it may be busy right now. Try again.',
    noSession: 'No live session with the speaker.',
    failed: 'That did not work — try again.',
    cannotSpeak: 'The speaker is connected, but the gateway cannot speak for it (relay mode or no voice rights).',
    needSession: 'A live session with the speaker is needed: say “Alice” and try again.',
};

export default en;
