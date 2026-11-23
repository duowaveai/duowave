// Deevi in the orbit's centre (docs/plans/mic-mode.md, «Главная без колонки»): in «Микрофон» mode there is no speaker,
// and the DuoWave mascot stands in its place — in the same frame and with the same light as the speakers' drawings
// (kit.tsx), so the dashboard shows «listens / thinks / speaks» the same way. The light under him is the colour of who
// works: lime at home, amber in the cloud.
//
// The figure is the mascot library (ui/mascot.tsx): its CURRENT character, Deevi today, so a new mascot appears here
// with no change in this file. Light body at once, rich (3D) when it loads and the frame is big enough. It follows the
// stage with the same moods; offline it sleeps. The frame keeps the click «показать путь вопроса» (DIVI_HIT) and says
// the label, so the figure takes no pointer events and is decoration for a screen reader.

import { useId } from 'preact/hooks';

import { tr } from '../i18n';
import { DIVI_BODY, diviEyes } from '@duowave/mascot/mark';
import type { MoodId } from '@duowave/mascot';
import { Mascot } from '../ui/mascot';
import { Defs, Frame, Light, Shadow, stageWord, ringSpot, skinOf, useReducedMotion } from './kit';
import type { StationArtProps, StationGlyphProps, StationStage } from './types';

const LIME = '#B6F36A';
const INK = '#0B0F0A';

/** Where a click on Deevi shows the question's path: his body, in frame coordinates. */
export const DIVI_HIT = { x: 52, y: 27, w: 136, h: 150, r: 56 };

const MOOD: Record<StationStage, MoodId> = { idle: 'idle', listen: 'listen', think: 'think', speak: 'speak' };

export function DiviArt({ ring, stage, offline }: StationArtProps) {
    const uid = useId().replace(/[^A-Za-z0-9_-]/g, '');
    const id = (name: string): string => `divi-${name}-${uid}`;
    const reduced = useReducedMotion();
    // The light: a platform under Deevi, steady at rest, shimmering at work, like the ring on a speaker's top.
    const spot = ringSpot(120, 176, 62, 9, 1, 1.5);
    return (
        <Frame
            label={tr.stations.diviLabel(offline ? tr.stations.offlineDivi : stageWord(stage))}
            overlay={<Mascot mood={offline ? 'sleep' : MOOD[stage]} interactive={false} shadow={false} label={false} class="st2-divi" />}
        >
            <Defs id={id} skin={skinOf('graphite')} />
            <Shadow rx={74} blur={`url(#${id('soft')})`} />
            <Light spot={spot} ring={ring} stage={stage} offline={offline} uid={uid} reduced={reduced} />
        </Frame>
    );
}

/** Deevi's icon in headers and lists: the whole mark, in its own colours. */
export function DiviGlyph({ size = 28 }: StationGlyphProps) {
    const [left, right] = diviEyes('open');
    return (
        <svg width={size} height={size} viewBox="0 0 256 256" aria-hidden="true">
            <path d={DIVI_BODY} fill={LIME} />
            {[left, right].map((eye, index) => (
                <path key={index} d={eye.d} fill="none" stroke={INK} stroke-linecap="round" stroke-width={eye.width} />
            ))}
        </svg>
    );
}
