// Texts: screens/helper-pc.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/helperPc';

const en: Translation<typeof ru> = {
    watchTitle: 'Watching over the speaker',
    reportTitle: 'Report for the developers',
    reportAbout: 'Something does not work — collect a report and send it to us. It holds the version, the system and the end of the log; passwords, keys, the home folder, your name and e-mail are taken out.',
    reportCollect: 'Collect the report',
    reportCopied: 'The report is copied — paste it into an e-mail or a message.',
    reportSelect: 'Could not copy it: select the text below and copy it.',
    modelsLabel: 'Models on this computer',
    hearingTitle: 'Hearing on this computer',
    hearingAbout: 'recognition models the speaker borrows from the computer',
    voicesTitle: 'Voices on this computer',
    voicesAbout: 'ready-made and your own, which the speaker uses through the computer',
};

export default en;
