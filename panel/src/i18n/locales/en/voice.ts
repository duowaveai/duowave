// Texts: voice/*.
import type { Translation } from '../../types';
import type ru from '../ru/voice';

const en: Translation<typeof ru> = {
    // creation.ts
    makeWidget: 'widget',
    makeGame: 'game',
    makeTheme: 'theme',
    makeSkill: 'skill',
    makeRequest: (what: string, words: string) => `Make a ${what}: ${words}`,
    agentCloud: 'agent — cloud',
    agentHome: 'agent — at home',
    agent: 'Agent',
    makingNew: (what: string) => `Making a ${what}. I’ll tell you when it’s ready — you can keep using the desk.`,
    makingAgain: (what: string) => `Redoing the ${what}. I’ll tell you when it’s ready — you can keep using the desk.`,
    readyLine: (title: string) => `Ready: “${title}”.`,
    readyHead: 'Agent · ready',
    listenMore: (seconds: number) => `listening ${seconds} s more`,
    chipFit: 'try it on',
    chipDesk: 'to the desk',
    chipEnable: 'turn on',
    chipRedo: 'redo…',
    chipPark: 'to drafts',
    chipDrop: 'no thanks',
    afterWidget: 'Stay quiet and the draft will wait here, on the desk.',
    afterTile: 'Stay quiet and the draft will wait here, on the desk as a tile.',
    knowStation: (phrase: string) => ` The speaker now knows it too: “${phrase}”.`,
    knowComputer: (phrase: string) => ` You can now say it too: “${phrase}”.`,
    done: 'Done',
    onDesk: (title: string, know: string) => `“${title}” is on the desk.${know}`,
    chipRemove: 'remove it',
    chipAgain: 'make another…',

    // flow.tsx
    failedHead: 'Failed',
    failedNote: (title: string) => `“${title}”: see what happened in the workshop.`,
    deleteDraftTitle: 'Delete the draft?',
    deleteDraftText: (title: string) => `“${title}” will leave the workshop.`,
    delete: 'Delete',

    // mic.ts
    micOff: 'Microphone off',
    micOffText: 'While a key field is open, the panel does not listen: personal things are typed by hand.',
    noMic: 'No microphone',
    listening: 'Listening',
    heardHere: 'heard by this computer',
    heardCloud: 'heard by the cloud',
    micFailed: 'The microphone did not open',
    listeningAnswer: 'Listening for the answer',
    notHeard: 'Didn’t hear anything',
    notHeardText: 'Press “Say” and speak — I listen until you pause.',
    recognizing: 'Recognizing…',
    notCaught: 'Didn’t catch that',
    notCaughtText: 'Say it again, a little louder.',
    heard: 'Heard',
    notRecognized: 'Could not recognize',
    failedTry: 'That didn’t work',
    notUnderstood: 'Not understood',
    notUnderstoodText: 'Say what is on the buttons, or “Make a widget: …”. What the panel did not understand goes nowhere.',
    textToSpeaker: 'Type to the speaker…',

    // speak.ts
    chipCloud: 'cloud',
    chipHome: 'at home',
    waitingAnswer: 'Waiting for your answer',
    answer: 'Answer',
    thinking: 'Thinking…',
    looking: (name: string) => `Checking: ${name}…`,
    answering: 'Answering',

    // strip.tsx
    hideStrip: 'Hide the strip',
    canSay: 'You can say:',
};

export default en;
