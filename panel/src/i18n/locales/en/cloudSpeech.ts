// Texts: screens/cloud-speech.tsx — the speaker's cloud hearing and voice without the computer.

import type { Translation } from '../../types';
import type ru from '../ru/cloudSpeech';

const en: Translation<typeof ru> = {
    hearingTitle: 'Cloud hearing',
    voiceTitle: 'Cloud voice',
    hearingAbout: 'The speaker recognizes speech in the cloud — without the computer and without Yandex. One phrase, one request.',
    voiceAbout: 'The speaker answers in a cloud voice — without the computer and without Yandex.',
    service: 'Service',
    openai: 'OpenAI',
    groq: 'Groq',
    custom: 'Own address',
    off: 'Off',
    model: 'Model',
    voice: 'Voice',
    url: 'Service address',
    urlHint: 'https://… /v1, OpenAI-compatible',
    keyOf: (service: string) => `${service} key`,
    keyShared: (service: string) => `The ${service} key is already here — the same as the “Brain” one.`,
    keySharedNote: 'This key is shared with “Brain”: a new one replaces it there too.',
    keyOwn: 'The key is saved.',
    keyOptional: 'Key, if the service asks for one',
    newKey: 'New key',
    save: 'Save',
    check: 'Check',
    works: 'It works.',
    ready: 'works',
    notReady: 'not set up',
    fromEnv: 'Set in the gateway’s .env. Save it here to manage it from the panel.',
    onStation: 'The key is kept on the speaker.',
    adminOnly: 'Only an administrator can set it up.',
    whereKey: 'Where to get a key',
    saved: 'Saved — works from the next phrase.',
};

export default en;
