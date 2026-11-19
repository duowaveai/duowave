// Texts: screens/auth.tsx — first setup and sign-in.

import type { Translation } from '../../types';
import type ru from '../ru/auth';

const en: Translation<typeof ru> = {
    passwordShort: 'The password needs at least 8 characters.',
    passwordsDiffer: 'The passwords do not match.',
    codeNeeded: 'Enter the setup code: the installer or the DuoWave log showed it.',
    nameNeeded: 'Enter your name.',
    welcome: 'Welcome to DuoWave',
    panel: 'Panel',
    setupSteps: 'First the administrator password: whoever sets it first manages the panel. Then where DuoWave will live and what it can do.',
    loginNamed: 'Your name and password — as the administrator entered you.',
    loginPassword: 'Enter the administrator password.',
    setupCode: 'Setup code',
    setupCodeHint: 'The “first panel setup” line in the DuoWave log: on a Mac — logs/duowave.log, in Docker — docker compose logs gateway.',
    setupCodeHintStation: (address: string) =>
        `The installer showed it at the end of installing on the speaker. Lost it — in the computer's Terminal: ssh -p 2222 root@${address} "grep 'первая настройка' /data/opt/duowave/gateway/gateway.log | tail -n 1"`,
    name: 'Name',
    adminPassword: 'Administrator password',
    password: 'Password',
    passwordPlaceholder: '8 characters or more',
    passwordAgain: 'Password again',
    wait: 'One moment…',
    saveAndEnter: 'Save and sign in',
    enter: 'Sign in',
    setupNoteCode: 'You are setting up from a computer other than the one running DuoWave, so a code is needed. The password is stored in DuoWave’s database as a hash.',
    setupNote: 'The password is stored in DuoWave’s database as a hash. You can add people and their rights later: “Settings” → “People and access”.',
    loginNote: 'The panel will remember this device for a month.',
};

export default en;
