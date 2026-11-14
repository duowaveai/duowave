// Texts: screens/placement-map.tsx (the “Where DuoWave runs” diagram).
import type { Translation } from '../../types';
import type ru from '../ru/placementMap';

const en: Translation<typeof ru> = {
    modelsTitle: 'Models and agents',
    modelsSub: 'ChatGPT, DeepSeek…',
    serviceTitle: 'Cloud service',
    alice: 'Alice',
    columnTitle: 'Speaker',
    columnSub: 'listens and speaks',
    computerTitle: 'Computer',
    pcOffSub: 'can be turned off',
    pcHelpsSub: 'helping',
    homeSubPc: 'speaker and computer on your network',
    captionPc: 'Hearing and voice stay home, on the computer. Only the request text goes out, to the model and the cloud service.',
    homeSubStation: 'just the speaker — the computer can be turned off',
    captionStation: 'The gateway runs on the speaker itself. Alice or the cloud hears and speaks, your pick; a cloud model thinks.',
    homeSubTogether: 'speaker and computer together',
    captionTogether: 'While the computer is online, hearing and voice run on it. When it sleeps, the cloud or Alice takes over, so the speaker never goes silent.',
    capEar: 'Hearing',
    capVoice: 'Voice',
    capConn: 'Connections',
    silent: 'The voice has nowhere to go: DuoWave is on the computer, and the computer is asleep.',
    mapLabel: (text: string) => `Diagram: where the voice goes. ${text}`,
    zoneHome: 'Home',
    zoneCloud: 'Cloud',
    zoneCloudSub: 'over the internet',
    legendHome: 'stays home',
    legendNet: 'over the internet',
    legendGateway: 'gateway',
    legendSettings: 'settings',
    hears: 'Hears',
    thinks: 'Thinks',
    speaks: 'Speaks',
};

export default en;
