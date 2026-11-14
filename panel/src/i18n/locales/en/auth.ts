// Texts: screens/auth.tsx — first setup and sign-in.

import type { Translation } from '../../types';
import type ru from '../ru/auth';

const en: Translation<typeof ru> = {
    passwordShort: 'The password needs at least 8 characters.',
    passwordsDiffer: 'The passwords do not match.',
    codeNeeded: 'Enter the setup code from the gateway log.',
    nameNeeded: 'Enter your name.',
    welcome: 'Welcome to DuoWave',
    panel: 'Panel',
    setupSteps: 'Step 1 of 2 — the administrator password: whoever sets it first manages the panel. Step 2 — how you will talk to DuoWave.',
    loginNamed: 'Your name and password — as the administrator entered you.',
    loginPassword: 'Enter the administrator password.',
    setupCode: 'Setup code',
    setupCodeHint: 'The “first panel setup” line in the gateway log: on a Mac — logs/duowave.log, in Docker — docker compose logs gateway.',
    name: 'Name',
    adminPassword: 'Administrator password',
    password: 'Password',
    passwordPlaceholder: '8 characters or more',
    passwordAgain: 'Password again',
    wait: 'One moment…',
    saveAndEnter: 'Save and sign in',
    enter: 'Sign in',
    setupNoteCode: 'You are setting up from another computer than the gateway’s, so a code is needed. The password is stored in the gateway’s database as a hash.',
    setupNote: 'The password is stored in the gateway’s database as a hash. You can add people and their rights later: “Settings” → “People and access”.',
    loginNote: 'The panel will remember this device for a month.',
};

export default en;
