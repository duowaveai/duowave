// Texts of Settings → Voice and Hearing (screens/voice-home.tsx) and of «a voice is waiting» on any screen
// (voice/pending-voice.tsx). The same keys as the Russian source.

import type { Translation } from '../../types';
import type ru from '../ru/voiceHome';

const voiceHome: Translation<typeof ru> = {
    pageVoice: 'Voice',
    pageVoiceSub: 'Who speaks, the model, your own voice',
    pageHearing: 'Hearing',
    pageHearingSub: 'Who listens, the model, the word',

    voiceLead: 'Whose voice the home answers in, which model it runs on, and how to make your own.',
    hearingLead: 'Who hears the questions, on which model, and which word the speaker answers to.',

    waitTitle: 'Voice is waiting',
    waitPill: (name: string) => `${name} is waiting for your decision`,
    waitPillFailed: (name: string) => `${name}: the voice did not start`,
    waitPillModel: (name: string, model: string) => `${name}: installing ${model}`,
    waitPillAnyModel: (name: string) => `${name}: installing the voice model`,
    waitPillFile: (name: string) => `${name}: downloading the voice`,
    waitPillImprint: (name: string) => `${name}: taking the voice`,
    waitPillPercent: (line: string, percent: number) => `${line} · ${percent} %`,
    waitDot: 'A voice is waiting for your decision',
    waitOn: (computer: string) => `on “${computer}”`,
    waitMeanwhile: 'Until you answer, the previous voice speaks.',
    waitLater: 'Decide later',
    waitAdminOnly: 'An administrator can answer.',

    nowTitle: 'Speaking now',
    nowAlice: 'Alice’s voice — the speaker speaks itself.',
    nowCloud: 'A cloud voice.',
    nowNone: 'No voice of its own — Alice’s voice answers.',
    nowVoice: (voice: string, where: string) => `“${voice}” — ${where}`,
    nowWhere: (computer: string) => `on “${computer}”`,
    nowHere: 'on this computer',
    nowComputer: (computer: string) => `The voice of the computer “${computer}”`,
    nowModel: (model: string) => `model ${model}`,
    nowCharacter: 'the character’s voice',

    modelTitle: (name: string) => `The voice of “${name}”`,
    modelSub: (computer: string) => `which model it runs on on “${computer}”`,
    modelNow: (engine: string, state: string) => `Now: ${engine} — ${state}`,
    modelApply: (engine: string) => `Move to ${engine}`,
    modelApplied: 'Saved: the model installs on the computer, and the voice switches by itself.',
    modelAway: 'The computer with the voice is offline — choose the model when it is back.',
    modelNone: 'The computer has no model this voice can run on: the fallback voice speaks.',

    ownHereOnly: 'Your own voice is recorded on the Turbo computer: open DuoWave there — Settings → Voice.',

    computerVoices: 'The computer’s voices',
    computerHearing: 'The computer’s hearing',

    hearNowTitle: 'Listening now',
    hearYandex: 'Yandex — the speaker listens itself, as out of the box.',
    hearCloud: 'Cloud recognition.',
    hearNone: 'No hearing of its own — Yandex listens.',
    hearModel: (model: string, where: string) => `${model} — ${where}`,
    hearComputer: (where: string) => `A computer — ${where}`,
    hearLanguages: (list: string) => `languages: ${list}`,

    wordTitle: 'Wake word',
    wordIs: (word: string) => `The speaker answers to “${word}”.`,
    wordDefault: (word: string) => `The speaker answers to “${word}” — as out of the box.`,
    wordAbout: 'A word of its own comes with the character. No words yet — find one in the store.',
    wordAboutMany: 'One word listens at a time: turn another on and the previous one goes off. A word that is off takes no RAM on the speaker.',
    wordAliceOnly: (word: string) => `Only “${word}”`,
    wordListening: 'listening',
    wordOff: 'off',
    wordTurnOn: 'Turn on',
    wordTurnOff: 'Turn off',
    wordFind: 'Find a word',
    wordRemoveAll: 'Remove all words',
    wordRemoveAllTitle: 'Remove all words?',
    wordRemoveAllText: 'The words are wiped from the speaker — both RAM and storage are freed. The speaker will answer to “Alice”. They can come back from the store.',
    wordOpen: 'Choose a character',

    briefTitle: 'Voice and hearing',
    briefLead: 'This computer’s voice and hearing models are in their own Settings items.',
    briefVoice: 'Voice',
    briefHearing: 'Hearing',
    briefVoiceAbout: 'who speaks, the model, your own voice',
    briefHearingAbout: 'the recognition model',
    briefOpen: 'Open',

    onbOwnTitle: 'Or — your own voice',
    onbOwnAbout: 'The speaker can answer in your voice: read a minute of text aloud, and the voice is ready in a minute.',
    onbOwnCreate: 'Create your voice',
    onbOwnOn: 'Done: the speaker speaks in your voice.',
    onbOwnStationMissed: (problem: string) => `The voice is ready, but the speaker did not switch: ${problem}. Switch it in Settings → Voice.`,
};

export default voiceHome;
