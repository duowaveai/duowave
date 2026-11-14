// ДиВи в центре орбиты (docs/plans/mic-mode.md, «Главная без колонки»): в режиме «Микрофон» колонки нет, и на её месте
// стоит знак DuoWave — в той же рамке и с тем же светом, что у рисунков колонок (kit.tsx), поэтому дашборд показывает
// «слушает / думает / говорит» так же. Слушает — глаза открыты; думает — смотрит в сторону; говорит — улыбается.
// Свет под ним — цвет того, кто работает: дома — лайм, облако — янтарь.

import { useId } from 'preact/hooks';

import { tr } from '../i18n';
import { DIVI_BODY, diviEyes } from '../ui/divi';
import { Defs, Frame, Light, Shadow, stageWord, ringSpot, skinOf, useReducedMotion } from './kit';
import type { StationArtProps, StationGlyphProps } from './types';

const LIME = '#B6F36A';
const INK = '#0B0F0A';
/** Знак 256×256 в рамке 240×200: тело — от y 31 до 172, по центру. */
const SCALE = 0.8;
const PLACE = `translate(${120 - 128 * SCALE} ${172 - 216 * SCALE}) scale(${SCALE})`;

/** Где у ДиВи нажимается «показать путь вопроса»: его тело, в координатах рамки. */
export const DIVI_HIT = { x: 52, y: 27, w: 136, h: 150, r: 56 };

export function DiviArt({ ring, stage, offline }: StationArtProps) {
    const uid = useId().replace(/[^A-Za-z0-9_-]/g, '');
    const id = (name: string): string => `divi-${name}-${uid}`;
    const reduced = useReducedMotion();
    const pose = offline ? 'open' : stage === 'think' ? 'look' : stage === 'speak' ? 'happy' : 'open';
    const eyes = diviEyes(pose);
    // Свет — площадка под ДиВи: ровный в покое, переливается в работе, как кольцо на крышке колонки.
    const spot = ringSpot(120, 176, 62, 9, 1, 1.5);
    return (
        <Frame label={tr.stations.diviLabel(offline ? tr.stations.offlineDivi : stageWord(stage))}>
            <Defs id={id} skin={skinOf('graphite')} />
            <Shadow rx={74} blur={`url(#${id('soft')})`} />
            <Light spot={spot} ring={ring} stage={stage} offline={offline} uid={uid} reduced={reduced} />
            <g transform={PLACE} class={`divi-art is-${offline ? 'offline' : stage}`}>
                <path d={DIVI_BODY} fill={offline ? '#C9CEC4' : LIME} />
                <g class="divi-eyes st2-anim">
                    {eyes.map((eye, index) => (
                        <path
                            key={index}
                            d={eye.d}
                            fill="none"
                            stroke={INK}
                            stroke-linecap="round"
                            stroke-width={eye.width}
                            opacity={offline ? 0.55 : 1}
                            style={{ transform: `translateX(${eye.dx}px)` }}
                        />
                    ))}
                </g>
            </g>
        </Frame>
    );
}

/** Значок ДиВи в шапке и списках: знак целиком, цвет — свой. */
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
