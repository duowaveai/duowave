// Texts: screens/local-asr.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/localAsr';

const en: Translation<typeof ru> = {
    backendMetal: 'the Mac’s GPU',
    backendVulkan: 'the GPU',
    backendCuda: 'the NVIDIA GPU',
    backendRocm: 'the AMD GPU',
    backendCpu: 'the CPU',
    backendThis: 'this computer',

    downloading: (done: number, total: number) => `downloading the model · ${done} of ${total} MB`,
    loadingWhile: (serving: string) => `loading a new model while ${serving} keeps working`,
    loading: 'loading the model…',
    readyHere: (where: string, device: string, fallback: string) => `running here, on ${where}${device}${fallback}`,
    gpuFailed: ' — the GPU failed',
    failedServing: (serving: string) => `${serving} is running; the new one did not start`,
    failed: 'not running',
    noModel: 'no model selected',

    titleFailed: 'Hearing did not start',
    titleSetup: 'Getting ready to listen',
    downloadingNote: 'Downloading the speech recognition model — once. After that it runs on this computer, offline.',
    progressLabel: 'Recognition model',
    notStarted: 'Recognition did not start.',
    listening: (serving: string) => `${serving} is listening for now.`,
    retrying: 'Trying…',
    retry: 'Retry',
};

export default en;
