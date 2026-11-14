// Texts: screens/ask.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/ask';

const en: Translation<typeof ru> = {
    title: 'Ask the Brain',
    aboutStation: 'the same model and tools as the speaker, without the speaker',
    aboutComputer: 'the same conversation as by voice, as text and with all connected servers',
    metaModel: 'model',
    meta: (model: string, provider: string, tools: number) => `${model}${provider ? ` · ${provider}` : ''} · MCP tools: ${tools}`,
    unreachable: 'The gateway is unreachable — try again.',
    questionLabel: 'Question',
    placeholder: 'For example: what do we have today in Todoist?',
    ask: 'Ask',
    restart: 'Start over',
    introStation:
        'Ask what you would ask the speaker: the model answers in words and, if needed, calls tools of the connected servers. Commands to the speaker itself are not run here.',
    introComputer:
        'Ask what you would ask by voice: the model answers in words and, if needed, calls tools of the connected servers. It is the same conversation as the “Say” button.',
    firstWord: (seconds: string) => `first word in ${seconds} s`,
    outsideText: 'text from the internet',
    waitingServer: 'waiting for the server',
};

export default en;
