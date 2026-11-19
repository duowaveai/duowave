// Texts: screens/say.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/say';

const en: Translation<typeof ru> = {
    title: 'Say',
    stationOnline: 'speaker online',
    stationOffline: 'speaker offline',
    placeholder: 'What should it say?',
    inputLabel: 'Text to say',
    sendLabel: 'Say',
    said: 'The speaker said it.',
    noAnswer: 'The speaker did not respond — it may be busy right now. Try again.',
    noSession: 'No live session with the speaker.',
    failed: 'That did not work — try again.',
    cannotSpeak: 'The speaker is connected, but the hub cannot speak for it (relay mode or no voice rights).',
    needSession: 'A live session with the speaker is needed: say “Alice” and try again.',
    voiceLabel: 'Whose voice',
    voiceOwn: 'Character’s voice',
    voiceAlice: 'Alice',
    aliceOnStation: 'Alice speaks only through the speaker',
    onStation: 'Plays on the speaker',
    onComputer: 'Plays on this computer',
    saying: 'Saying…',
    saidHere: 'Said.',
    noOwnVoiceHere: 'This computer has no voice of its own: choose one in “Settings → Voice”.',
    cardTitle: 'Say',
    cardSub: 'Type a text — the character’s voice or Alice will say it',
};

export default en;
