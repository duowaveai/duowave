// Texts: media/* — the panel's one viewer and player (photos, videos, sound) and the media engine's keys.

import type { Translation } from '../../types';
import type ru from '../ru/media';

const en: Translation<typeof ru> = {
    viewerLabel: 'Viewer',
    close: 'Close',
    previous: 'Back',
    next: 'Next',
    counter: (n: number, of: number) => `${n} of ${of}`,
    fullscreen: 'Full screen',
    share: 'Share',
    original: 'Download the original',
    opening: 'Opening…',
    notShown: 'The browser does not show this file — you can download it.',
    notPlayed: 'The browser does not play this file — you can download it.',
    failed: 'could not open it',
    noViewer: 'the panel’s viewer is not here',
    nothing: 'nothing to show',
    ownPath: 'a path to a file of its own folder is needed, for example episodes/01.mp4',
    draftNoFolder: 'a draft has no folder of its own',
};

export default en;
