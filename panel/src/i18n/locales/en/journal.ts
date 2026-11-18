// Texts: screens/journal.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/journal';

const en: Translation<typeof ru> = {
    keepNone: 'Do not keep',
    keepWeek: 'A week',
    keepMonth: 'A month',
    keepYear: 'A year',
    keepDays: (days: number) => `${days} d`,
    noSessions: 'The hub did not return sessions — try again later.',

    stationTitle: 'Speaker',
    disconnected: (ago: string) => `disconnected ${ago}`,
    connected: (ago: string) => `connected ${ago}`,
    firmware: 'firmware',
    events: 'events',
    canSpeak: 'can speak',
    yandexOn: 'Yandex connected',
    yandexOff: 'No Yandex',

    noWords: 'No words',
    quote: (text: string) => `“${text}”`,
    quotePartial: (text: string) => `“${text}…”`,
    playAt: (time: string) => ` at ${time}`,
    noSteps: 'No steps: the request ended before it began.',

    keepLabel: 'Keep the journal',
    keepAria: 'How long to keep the request journal',
    deleteAllNow: 'The whole request journal will be deleted now.',
    deleteOlderNow: (days: number) => `Requests older than ${days} d will be deleted now.`,
    delete: 'Delete',
    cancel: 'Cancel',

    requests: 'Requests',
    notKept: 'not kept',
    keptFor: (days: number) => `kept for ${days} d`,
    emptyStation: 'Nothing yet: ask the speaker — the request shows up here while it answers.',
    emptyComputer: 'Nothing yet: press “Say” and ask — the request shows up here while DuoWave answers.',
    loading: 'Loading…',
    more: 'More',

    noRightStation: 'What was said to the speaker is visible to anyone the administrator gave journal rights.',
    noRightComputer: 'What was said to DuoWave is visible to anyone the administrator gave journal rights.',
    unsupported: 'Not supported yet',
    unsupportedEmpty: 'Empty: everything that was asked for was handled by someone.',
    opened: (activator: 'word' | 'button' | 'answer' | 'text', name?: string) =>
        activator === 'word' ? `by the word “${name ?? '…'}”` : activator === 'button' ? 'by the button' : activator === 'answer' ? 'an answer to a question' : 'typed',
    firstWord: (time: string) => `first word in ${time}`,
    tokens: (input: number, output: number, cached: number, rounds: number) =>
        `tokens ${input} → ${output}${cached ? `, ${cached} from the cache` : ''}${rounds > 1 ? `, ${rounds} rounds` : ''}`,
    deal: (seat: string, model: string, money: string) => [seat, model, money].filter(Boolean).join(' · '),
};

export default en;
