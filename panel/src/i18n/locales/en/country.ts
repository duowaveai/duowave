// Texts: screens/country.tsx — the home's country.
import type { Translation } from '../../types';
import type ru from '../ru/country';

const en: Translation<typeof ru> = {
    askTitle: 'Where is your home?',
    askAbout:
        'By the country DuoWave decides what on the internet works from here directly and what is better sent through your server. Guessed from this computer’s time zone and language — never from your address; the country stays on the gateway.',
    other: 'Another country…',
    wontSay: 'Rather not say',
    pick: 'Choose a country',
    search: 'Find a country',
    cardTitle: 'Home country',
    cardWhere: 'internet access, apps',
    cardAbout: 'What on the internet works from here directly. It never leaves the gateway.',
    notAsked: 'Not chosen',
    none: 'Rather not say — nothing is chosen by country',
    onlyAdmin: 'Whoever can change the settings can change it.',
    saved: 'Saved',
};

export default en;
