// Deevi, DuoWave's first mascot: the logo mark as a character. A lime rounded body with two eyes; a flat SVG body that
// shows at once and a 3D body (three.js, its own ~142 KB gzip chunk) for big boxes. Both draw one rig (rig.ts).

import { defineCharacter } from '../../engine/mount.ts';
import { createFlat, stillFlat } from './flat.ts';
import { DEEVI_COLORS, LOOKS, TUNES } from './looks.ts';
import { deeviRig } from './rig.ts';

export const deevi = defineCharacter({
    id: 'deevi',
    colors: DEEVI_COLORS,
    looks: LOOKS,
    tunes: TUNES,
    rig: deeviRig,
    light: { create: createFlat },
    still: stillFlat,
    rich: {
        load: () => import('./scene3d.ts').then((module) => module.create3d),
        drag: true,
        // 3D at icon size is waste; a page has room for about 16 WebGL contexts.
        minSize: 96,
        maxLive: 4,
    },
});

export { DEEVI_COLORS } from './looks.ts';
