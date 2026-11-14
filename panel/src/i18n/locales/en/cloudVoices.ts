// Texts: screens/voice-providers.tsx — «Cloud voices» in the voice tab: the voices' providers for apps.

import type { Translation } from '../../types';
import type ru from '../ru/cloudVoices';

const en: Translation<typeof ru> = {
    title: 'Cloud voices',
    forApps: 'for apps',
    intro: 'Apps voice their lines with the computer’s voices, and with a key — with cloud ones too. The lines’ text goes to the provider and spends your plan, so each app asks for your “yes” first.',
    keySet: (hint: string) => `key set · …${hint}`,
    noKey: 'no key',
    cloud: 'cloud',
    paid: 'paid',
    connected: 'connected',
    check: 'check',
    works: (voices: number) => `Works: ${voices} voices`,
    left: (left: string, limit: string, unit: string, tier: string | null) =>
        `${left} ${unit} left of ${limit}${tier ? ` · plan ${tier}` : ''}`,
    newKey: 'New key',
    keyOf: (title: string) => `${title} key`,
    save: 'save',
    removeKey: 'remove key',
    whereKey: 'Where to get a key',
    keyByPeople: 'A person with the right to connections pastes the key.',
    removeTitle: (title: string) => `Remove the ${title} key?`,
    removeText: 'Apps will stop speaking with its voices.',
    remove: 'Remove',
    removed: 'Key removed.',
};

export default en;
