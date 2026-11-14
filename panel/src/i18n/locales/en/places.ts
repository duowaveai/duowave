// Texts: screens/places.tsx (“Settings”: the page list and the pages).
import type { Translation } from '../../types';
import type ru from '../ru/places';

const en: Translation<typeof ru> = {
    talkTitle: 'How to talk to DuoWave',
    readyTitle: 'Readiness',
    micTitle: 'Microphone',
    nameTitle: 'Name and look',
    nameAbout: 'what the speaker is called in the panel and in the top bar; the look is its model and color',
    change: 'Change',
    modeTitle: 'Mode',

    backAskTitle: 'Give the speaker back to Yandex?',
    backAskText: 'Yandex will answer everything, as before DuoWave: your own character, voice and skills will go quiet. To bring them back, pick the “Together” or “Own” mode right here.',
    backAskOk: 'Give back to Yandex',
    backDone: 'Yandex runs the speaker. To bring your own back, use the mode above.',
    backTitle: 'Give the speaker back to Yandex',
    backAbout: 'If something went wrong or you no longer need DuoWave: the speaker is fully Yandex’s again. DuoWave deletes nothing.',
    backBusy: 'Giving back…',

    helperTitle: 'Helper computer',
    helpTitle: 'Help the speaker',
    watchTitle: 'Watching over the speaker',
    copiesTitle: 'Backups and safety net',

    devTitle: 'For developers',
    workshop: 'Workshop',
    workshopSub: 'your own skills and widgets; in the dock while the layer is on',
    routesTitle: 'Who answers, as a table',
    routesSub: 'routes: own, Yandex, in order',
    modelsTitle: 'Models and providers',
    modelsSub: 'keys, models from .env, fast and smart',
    mcpTitle: 'Programs (MCP)',
    mcpSub: 'tool servers for the agent',
    journalTitle: 'Request journal',
    journalSub: 'the steps of each question, timing, who answered',
    keysTitle: 'API keys',
    keysSub: 'for your own programs and Home Assistant',
    showDev: 'Show the developer layer',
    showDevAbout: '“Workshop” in the dock, request journal, routes as a table, providers and keys',
    devLayer: 'Developer layer',
    devPlaces: 'Developer places',

    pageMic: 'Microphone',
    pageMicSub: 'how to talk, hearing, check',
    pageStation: 'Speaker',
    pageStationSub: 'name, mode, hearing, where it runs',
    pageHelperSub: 'what it gives the speaker, stay awake',
    pageAccess: 'People and access',
    pageAccessSub: 'who signs in, passwords, your own sign-in',
    pageLook: 'Appearance',
    pageLookSub: 'theme, light and dark, look',
    pageUpdates: 'Updates and backups',
    pageUpdatesSub: 'DuoWave version, settings backups',
    pageHistory: 'Conversation history',
    pageHistorySub: 'what was asked and who answered',
    pageDevSub: 'workshop, routes, providers',
    settings: 'Settings',
    newVersion: 'new version',
};

export default en;
