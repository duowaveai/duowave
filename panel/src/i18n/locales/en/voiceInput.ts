// Texts: screens/voice-input.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/voiceInput';

const en: Translation<typeof ru> = {
    toComputerTitle: 'Talk with this computer’s microphone?',
    toComputerText:
        'Alice, Yandex and the speaker will leave the panel, and the mode becomes “Own”. There is no music, radio, timers or alarms on the computer yet. To undo it: “Settings” → “Microphone” → “Through the speaker with Alice”.',
    toComputerOk: 'Switch to the microphone',
    toStationTitle: 'Bring back Alice and the speaker?',
    toStationText: 'The panel will show the speaker, Alice and Yandex again, and the mode returns to what it was. You can pick this computer’s microphone again — right here.',
    toStationOk: 'Bring back',

    optionComputerTitle: 'With this computer’s microphone',
    optionComputerSub: 'the “Say” button and ⌥Space; answers in the computer’s voice, no speaker and no Yandex',
    optionStationTitle: 'Through the speaker with Alice',
    optionStationSub: 'The Yandex Station listens for the word “Alice”; DuoWave decides who answers',
    howToTalk: 'How to talk with DuoWave',
    chosen: 'selected',
    adminChooses: 'How to talk with DuoWave is chosen by the administrator.',

    readinessLabel: 'Readiness',
    hearTitle: 'Hearing',
    hearPreparing: 'getting ready: the recognition model is loading',
    hearOk: 'this computer recognizes speech',
    hearOff: 'recognition is not set up',
    hearFix: 'Set up hearing',
    voiceTitle: 'Voice',
    voiceChecking: 'checking…',
    voiceNotChosen: 'no own voice selected',
    voiceReady: 'own voice is ready',
    voiceStarting: 'the voice is starting…',
    voiceFix: 'Choose a voice',
    brainTitle: 'Brain',
    brainChosen: 'model selected',
    brainNone: 'no model selected',
    brainFix: 'Choose a model',
    allReady: 'All set: press “Say” and ask something.',
    whenReady: 'When everything is checked, press “Say” and ask something.',

    micUnavailable: 'The microphone is unavailable.',
    micCheckTitle: 'Microphone check',
    micCheckOn: 'Say something: the bar shows how well you are heard.',
    micHeard: 'The microphone hears you.',
    micCheckIdle: 'Five seconds of sound level. Nothing is recorded.',
    stop: 'Stop',
    check: 'Check',
    micLevel: 'Microphone level',

    sheetComputerTitle: 'Talking with this computer’s microphone',
    sheetComputerAbout: 'Step 2 of 2 — what is ready',
    sheetTitle: 'How will you talk with DuoWave?',
    sheetAbout: 'Step 2 of 2 — you can change it later in “Settings”',
    later: 'Decide later',

    micFirstTitle: 'Or talk with this computer’s microphone',
    micFirstText: 'No speaker needed: press “Say”, ask — DuoWave answers in the computer’s voice. Memory, apps and character stay the same.',
    enabling: 'Turning on…',
    talkWithMic: 'Talk with the microphone',
};

export default en;
