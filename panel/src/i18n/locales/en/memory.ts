// Texts: screens/memory.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/memory';

const en: Translation<typeof ru> = {
    pressureNormal: 'pressure normal',
    pressureWarn: 'memory is low',
    pressureCritical: 'memory is very low',
    segSystem: 'System and Alice',
    segGateway: 'DuoWave',
    segPlugins: 'Apps',
    segFree: 'Free',
    title: 'Speaker resources',
    mb: (n: number) => `${n} MB`,
    budget: (budget: number, reserve: number) =>
        `App budget: ${budget} MB. An app takes its share only while loaded; idle ones are unloaded to make room. Reserve for the system and Alice: ${reserve} MB.`,
    actionWarn: ' Under heavy pressure apps are not turned off — you only get a warning.',
    actionOff: ' Under heavy pressure the last enabled of the loaded apps turns itself off.',
    disk: (free: number, packages: number, reserve: number) =>
        `Storage: ${free} MB free, apps take ${packages} MB, ${reserve} MB is reserved for the gateway update.`,
    pictures: (used: string, limit: string | null) => `App pictures: ${used} MB${limit ? ` of ${limit}` : ''}.`,
    lineMemory: (used: string, budget: number) => `memory ${used} of ${budget} MB`,
    lineBudget: (budget: number) => `budget ${budget} MB`,
    lineCalls: (n: number) => `calls ${n}`,
    lineErrors: (n: number) => `errors ${n}`,
};

export default en;
