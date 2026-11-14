// Texts: screens/routes.tsx.
import { plural } from '../../format';
import type { Translation } from '../../types';
import type ru from '../ru/routes';

const en: Translation<typeof ru> = {
    whoAnswers: 'Who answers',
    pillOwn: 'own',
    pillOwnYandex: 'own + Yandex',
    pillYandex: 'Yandex',
    pillYandexOwn: 'Yandex + own',
    pillOff: 'off',
    noteOwn: 'only our code answers',
    noteOwnYandex: 'ours first; Yandex picks up what it does not know',
    noteYandex: 'handed to Yandex entirely',
    noteOff: 'nobody: the request lands in “Not supported yet”',
    waitingGateway: 'Waiting for the gateway…',
    search: 'Search',
    searchLabel: 'Search capabilities',
    changeSuffix: ', change',
    served: (mine: number, yandex: number) => `by us — ${mine} · by Yandex — ${yandex}`,
    noRequests: 'no requests yet',
    noOwnImplementation: 'no own implementation',
    nothingFound: 'Nothing found.',
    relayNote: 'The mode is “Yandex” now: this takes effect in “Together” and “Own”. ',
    ownNote: 'The mode is “Own” now: “Yandex” steps are skipped, and in “Together” they work as they are. ',
    mayEditNote: 'Tap a row to choose who answers — it applies at once, no restart. ',
    adminOnlyNote: 'Only the administrator can choose who answers. ',
    envDefault: 'ROUTES in .env stays the default for a new database',
    envChanged: '; changes from the panel are in effect.',
    envIsDefault: '; the default is in effect now.',
    count: (n: number) => `${n} ${plural(n, { one: 'capability', other: 'capabilities' })}.`,
    sheetLabel: (title: string) => `Who answers: ${title}`,
    close: 'Close',
    withoutYandexInOwn: ' — no Yandex in “Own”',
    willBecome: (to: string, from: string) => `Will become “${to}” instead of “${from}”`,
    applying: 'Applying…',
    apply: 'Apply',
    cancel: 'Cancel',
    adminOnly: 'Only the administrator can choose who answers.',
};

export default en;
