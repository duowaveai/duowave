// Texts: screens/house.tsx.
import { plural } from '../../format';
import type { Translation } from '../../types';
import type ru from '../ru/house';

const en: Translation<typeof ru> = {
    pillApproved: 'answering',
    pillPending: 'waiting for approval',
    pillBlocked: 'blocked',
    blockTitle: (name) => `Block “${name}”?`,
    blockText: 'It will stop answering until you allow it again.',
    blockOk: 'Block',
    stations: 'Speakers',
    stationsCount: (n) => `${n} ${plural(n, { one: 'speaker', other: 'speakers' })}`,
    noStations: 'No speaker has connected yet.',
    addressUnknown: 'address unknown',
    lastSeen: (address, ago) => `${address} · seen ${ago}`,
    allow: 'allow',
    block: 'block',
    feedNoAnswer: (error) => `The feed did not answer: ${error}`,
    latestInstalled: 'The latest version is installed.',
    installTitle: (version) => `Install version ${version}?`,
    installTextStation: 'The gateway will restart, and the speaker will reconnect on its own.',
    installTextDevice: 'The gateway will restart, and the panel will wait for it on its own.',
    installOk: 'Install',
    updates: 'Updates',
    version: (version) => `version ${version}`,
    urgent: 'Urgent update',
    urgentText: ' — fixes something important in this version.',
    urgentAuto: 'The gateway will install it on its own within an hour; you can do it now.',
    urgentManual: 'Install it as soon as possible.',
    availableVersion: 'Version available: ',
    noFeed: 'No release feed is set: the gateway does not check for updates.',
    checking: 'Checking for a new version…',
    noNewVersions: (ago) => `No new versions · checked ${ago}`,
    notChecked: 'Not checked yet.',
    installing: 'Installing. The gateway will restart and be back in a few seconds.',
    check: 'check',
    installingShort: 'installing…',
    update: 'update',
};

export default en;
