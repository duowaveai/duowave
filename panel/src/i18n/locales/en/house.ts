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
    installTextStation: 'DuoWave will restart, and the speaker will reconnect on its own.',
    installTextDevice: 'DuoWave will restart, and the panel will wait for it on its own.',
    installTextFromComputer: 'The computer that runs the speaker will install it: it takes a copy of the settings, tries the new version from RAM and restarts DuoWave. The speaker reconnects on its own.',
    fromComputerStarted: 'The computer is installing the new version on the speaker — about a minute.',
    installOk: 'Install',
    updates: 'Updates',
    version: (version) => `version ${version}`,
    urgent: 'Urgent update',
    urgentText: ' — fixes something important in this version.',
    urgentAuto: 'DuoWave will install it on its own within an hour; you can do it now.',
    urgentManual: 'Install it as soon as possible.',
    availableVersion: 'Version available: ',
    noFeed: 'No release feed is set: DuoWave does not check for updates.',
    checking: 'Checking for a new version…',
    noNewVersions: (ago) => `No new versions · checked ${ago}`,
    notChecked: 'Not checked yet.',
    installing: 'Installing. DuoWave will restart and be back in a few seconds.',
    trialTrying: (version, until) =>
        `Version ${version} is being tried from the speaker’s memory until ${until}. If it falls, the speaker goes back to the previous one by itself.`,
    trialWaiting: (version) =>
        `Version ${version} passed its trial and took the previous one’s place: DuoWave will restart for a couple of seconds once the speaker is quiet.`,
    trialStuck: (version, why) => `Version ${version} runs from memory but is not on the disk yet: ${why}`,
    trialAppsPaused: 'The speaker’s apps are off for the trial and will come back on by themselves.',
    check: 'check',
    installingShort: 'installing…',
    update: 'update',
};

export default en;
