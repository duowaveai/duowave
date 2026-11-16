// Texts: screens/home.tsx.
import { plural } from '../../format';
import type { Translation } from '../../types';
import type ru from '../ru/home';

const en: Translation<typeof ru> = {
    modeRelay: 'Alice',
    modeGateway: 'Alice + your AI',
    modeOwn: 'Your AI',
    modeUnavailable: 'This mode can’t be turned on here right now.',
    modeGroup: 'Who answers',
    switching: 'Switching — the speaker will reconnect in a few seconds…',
    reconnecting: 'The speaker is reconnecting in the new mode…',
    collapse: 'collapse',
    more: 'more',
    adminSwitches: ' An administrator switches this.',
    yandexEar: 'Yandex hearing',
    yandexEarNote: 'With no hearing of your own, Yandex recognizes the question and your brain answers. The voice goes to Yandex’s cloud for that.',
    autoNow: (mode, reason, since, from) => `Now “${mode}”: ${reason} since ${since}. I’ll go back to “${from}” when it’s available again.`,
    stayIn: (mode) => `Stay in “${mode}”`,
    warnTitle: 'Alice’s voice is chosen — it won’t sound without Yandex',
    warnText: 'In “Your AI” mode, our voice will speak instead (or the speaker will go silent if there is no own voice server). Choose your own voice — or continue like this.',
    pickOwnVoice: 'Choose your own voice',
    proceed: 'Continue',
    cancel: 'Cancel',
    whoAnswered: 'Who answered how much',
    relayObserves: '“Alice” mode: Alice answers, DuoWave only observes.',
    warmingUp: 'DuoWave is warming up — this will appear in a second.',
    legendOwn: 'own',
    legendYandex: 'Yandex',
    legendOff: 'off',
    capabilities: (n) => `${n} ${plural(n, { one: 'capability', other: 'capabilities' })}`,
    sayToStation: 'Say something to the speaker — who answered will appear here.',
    servedSince: (own, yandex) => `served since start: by us — ${own} · by Yandex — ${yandex}`,
    couldNot: (n) => ` · couldn’t handle — ${n}`,
    recentRequests: 'Recent requests',
    conversationHistory: 'Conversation history',
    notYet: 'Not supported yet',
    allCount: (n) => `All ${n}`,
    quoted: (text) => `“${text}”`,
};

export default en;
