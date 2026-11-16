// Texts of the first-steps step “Who thinks” (screens/onb/brain.tsx).
import type { Translation } from '../../types';
import type ru from '../ru/onbBrain';

const en: Translation<typeof ru> = {
    title1: 'Who will think',
    title2: 'about the answers?',
    lead: 'DuoWave is a home for AI, not an AI itself. Connect the model you like: you pay its service directly.',
    groupLabel: 'Who thinks about the answers',

    orTitle: 'OpenRouter',
    orChip: 'Easiest',
    orSub: 'One sign-in, many models: ChatGPT, Claude, Gemini, DeepSeek. You pay OpenRouter for what you use.',
    gptTitle: 'ChatGPT subscription',
    gptSub: 'Have ChatGPT Plus or Pro? Sign in with the same account and answers come from your subscription.',
    keyTitle: 'Your own key',
    keySub: 'An API key from OpenAI, Anthropic, DeepSeek, YandexGPT or another service.',
    localTitle: 'A model on this computer',
    localChip: 'offline',
    localSub: 'Ollama or LM Studio: questions never leave home. Needs a powerful computer.',
    localSubAway: 'Ollama or LM Studio: a model on this computer answers. Needs a powerful computer.',

    orSignIn: 'Sign in with OpenRouter',
    orSignInNote: 'The OpenRouter page opens: sign in and press “Authorize”. The key arrives by itself, nothing to copy.',
    orDone: 'Signed in to OpenRouter',
    gptSignIn: 'Sign in with ChatGPT',
    gptSignInNote: 'The OpenAI sign-in opens. Works with Plus and Pro.',
    gptDone: 'Signed in to ChatGPT',
    chosenNote: 'The fast model is for short answers, the smart one for widgets and games. We picked them for you; to change, go to “Character” → “Brain”.',
    opening: 'Opening…',
    checking: (title: string) => `Checking ${title}…`,
    checkingNote: 'Asking the model a short question, a few seconds.',

    keyLabel: 'API key',
    keyPlaceholder: 'sk-…',
    keyCheck: 'Check',
    keyChecking: 'Checking…',
    keyNote: 'I can tell the service from the key. The key stays on this computer.',
    keyKnown: (title: string) => `This is a ${title} key. The key stays on this computer.`,
    keyUnknown: 'I can’t tell the service from this key. Add it in “Character” → “Brain”, where you can pick the service yourself.',
    keyDone: (title: string) => `The ${title} key works`,

    localSearching: 'Looking for Ollama and LM Studio…',
    localSearchingNote: 'Checking whether one of them is running on this computer.',
    localAgain: 'Check again',
    localNone: 'Ollama and LM Studio not found',
    localNoneNote: 'Install one of them and press “Check again”. Or start with the cloud and add a model on this computer later.',
    localFound: (title: string) => `Found ${title}`,
    localFoundNote: (model: string) => `${model} answers, and questions never leave home. We picked it for you; to change, go to “Character” → “Brain”.`,
    localFoundBackup: (model: string, title: string) =>
        `${model} answers. When the model on this computer is silent, the question goes to the backup, “${title}”, over the internet. To change, go to “Character” → “Brain”.`,
    localFoundSmart: (model: string, title: string) =>
        `${model} answers; hard questions go to “${title}” over the internet. To change, go to “Character” → “Brain”.`,
    noBackup: 'No backup',

    earTitle: 'Getting ready to listen',
    earDownloading: 'downloading the speech model once, then it works offline',
    earLoading: 'loading the speech model',
    earFailed: 'Hearing did not start',
    earSize: (done: number, total: number) => `${done} of ${total} MB`,
    earProgress: 'Downloaded',

    later: 'Decide later',
    noModelNote: 'Without a choice DuoWave can’t answer yet. I’ll remind you on the desk.',
};

export default en;
