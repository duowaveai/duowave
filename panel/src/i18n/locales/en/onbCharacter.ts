// Texts of the first-steps step «Character» (screens/onb/character.tsx).
import type { Translation } from '../../types';
import type ru from '../ru/onbCharacter';

const en: Translation<typeof ru> = {
    title1: 'Who will',
    title2: 'talk with you?',
    lead: 'A character is a personality and a voice. The model stays the same, the manner changes. Switch any time: “Character” in the dock below.',
    more: 'More characters in MagicPlace: install with one tap, remove the same way.',
    noteStation: 'The chosen character answers “Alice, …”; music and alarms stay with Alice.',
    noteComputer: 'The character speaks through the computer: its speakers or your headphones.',
    listen: 'Listen',
    playing: 'Playing…',
    listenTo: (name: string) => `Listen: ${name}`,
    stop: 'Stop',
    quoted: (text: string) => `“${text}”`,
    reading: 'Reading characters…',
    none: 'No characters yet: you can install them in MagicPlace.',
    choosing: (name: string) => `Choosing ${name}…`,
    chosen: (name: string) => `${name} answers from the next question.`,
    onlyEditors: 'The character is chosen by someone allowed to change the personality.',
    pickAnyway: 'Memory is tight. Tap the character again to choose it anyway.',
    stationMissed: (problem: string) => `The voice is chosen, but the speaker kept its character: ${problem}`,
};

export default en;
