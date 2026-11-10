/**
 * Знак DuoWave — ДиВи (утверждён 08.11.2026, DESIGN.md): лаймовый скруглённый квадрат и два тёмных
 * глаза. В статике ДиВи всегда подмигивает: левый глаз — капсула, правый — дужка. Холст 256×256, как у исходных
 * файлов знака. Три варианта:
 * - color — основной: тело лайм #B6F36A, глаза #0B0F0A, в любой теме; шапка, схема;
 * - mono — тело цвета текста, глаза вырезаны (фон просвечивает): подпись «Powered by DuoWave»;
 * - clear — тело 14% цвета текста, глаза цвета текста: на цветном фоне и где лайм лишний.
 * Надпись «duowave» — прежние контуры сайта (svg.lp-wordmark, viewBox 944×224), цвета текста.
 */

import type { RefObject } from 'preact';
import { useLayoutEffect, useRef } from 'preact/hooks';

import { mountDivi } from './divi';

export type DuoWaveVariant = 'color' | 'mono' | 'clear';

/** Тело: скруглённый квадрат 160×176, радиус 68. */
const BODY = 'M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z';
/** Глаза подмигивания: капсула (x 98) и дужка (x 158), линии уже переведены в контуры. */
const WINK = 'M81.2 110A16.8 16.8 0 0 1 114.8 110V138A16.8 16.8 0 0 1 81.2 138ZM132.91 123.5A30.32 30.32 0 0 1 183.09 123.5A9.8 9.8 0 0 1 166.87 134.5A10.72 10.72 0 0 0 149.13 134.5A9.8 9.8 0 0 1 132.91 123.5Z';

const LIME = '#B6F36A';
const INK = '#0B0F0A';

/**
 * Живой ДиВи (ui/divi.ts, DIVI.md): при загрузке подмигивает, при наведении на `hover` (логотип целиком) или на сам знак
 * радуется и подпрыгивает. SVG рисует и двигает divi.ts — Preact не трогает содержимое этого span.
 */
function LiveMark({
    size,
    title,
    hover,
    idle,
    cheer,
}: {
    size: number;
    title?: string;
    hover?: RefObject<HTMLElement>;
    idle?: boolean | 'lively';
    cheer?: boolean;
}) {
    const host = useRef<HTMLSpanElement>(null);
    useLayoutEffect(() => {
        if (!host.current) return;
        return mountDivi(host.current, { intro: !cheer, cheer, idle, hover: hover?.current ?? undefined }).destroy;
    }, []);
    return (
        <span
            ref={host}
            class="duowave-mark duowave-live"
            style={{ width: `${size}px`, height: `${size}px` }}
            role={title ? 'img' : undefined}
            aria-label={title}
            aria-hidden={title ? undefined : true}
        />
    );
}

export function DuoWaveMark({
    size = 28,
    variant = 'color',
    title,
    live = false,
    idle,
    cheer,
}: {
    size?: number;
    variant?: DuoWaveVariant;
    title?: string;
    /** Живой знак шапки: подмигивает при загрузке, радуется при наведении. Только у цветного. */
    live?: boolean;
    /** Живому знаку — оживать самому: `'lively'` у входа и мастера подключения (ui/divi.ts). */
    idle?: boolean | 'lively';
    /** Живому знаку — обрадоваться при появлении: колонка подключилась. */
    cheer?: boolean;
}) {
    if (live && variant === 'color') return <LiveMark size={size} title={title} idle={idle} cheer={cheer} />;
    return (
        <svg
            class="duowave-mark"
            width={size}
            height={size}
            viewBox="0 0 256 256"
            role={title ? 'img' : undefined}
            aria-label={title}
            aria-hidden={title ? undefined : true}
        >
            {variant === 'color' && (
                <>
                    <path d={BODY} fill={LIME} />
                    <path d={WINK} fill={INK} />
                </>
            )}
            {variant === 'mono' && <path d={BODY + WINK} fill="currentColor" fill-rule="evenodd" />}
            {variant === 'clear' && (
                <>
                    <path d={BODY} fill="currentColor" fill-opacity="0.14" />
                    <path d={WINK} fill="currentColor" />
                </>
            )}
        </svg>
    );
}

/** «duowave»: контуры надписи сайта, по отдельности (часть букв пересекается, одним контуром evenodd их не сложить). */
const WORDMARK: { d: string; evenodd?: boolean }[] = [
    { d: 'M50 49H66A50 50 0 0 1 116 99V125A50 50 0 0 1 66 175H50A50 50 0 0 1 0 125V99A50 50 0 0 1 50 49ZM50 73H66A26 26 0 0 1 92 99V125A26 26 0 0 1 66 151H50A26 26 0 0 1 24 125V99A26 26 0 0 1 50 73Z', evenodd: true },
    { d: 'M92 0H116V172H92V0Z' },
    { d: 'M138 52V125A50 50 0 0 0 188 175H204A50 50 0 0 0 254 125V52H230V125A26 26 0 0 1 204 151H188A26 26 0 0 1 162 125V52Z' },
    { d: 'M230 52H254V172H230V52Z' },
    { d: 'M326 49H342A50 50 0 0 1 392 99V125A50 50 0 0 1 342 175H326A50 50 0 0 1 276 125V99A50 50 0 0 1 326 49ZM326 73H342A26 26 0 0 1 368 99V125A26 26 0 0 1 342 151H326A26 26 0 0 1 300 125V99A26 26 0 0 1 326 73Z', evenodd: true },
    { d: 'M406 52H428V136.5A16.5 16.5 0 0 0 461 136.5V76H483V136.5A16.5 16.5 0 0 0 516 136.5V52H538V136.5A38.5 38.5 0 0 1 472 163.44A38.5 38.5 0 0 1 406 136.5Z' },
    { d: 'M602 49H618A50 50 0 0 1 668 99V125A50 50 0 0 1 618 175H602A50 50 0 0 1 552 125V99A50 50 0 0 1 602 49ZM602 73H618A26 26 0 0 1 644 99V125A26 26 0 0 1 618 151H602A26 26 0 0 1 576 125V99A26 26 0 0 1 602 73Z', evenodd: true },
    { d: 'M644 52H668V172H644V52Z' },
    { d: 'M690 52L715.49 52L748 142.71L780.51 52L806 52L763 172L733 172Z' },
    { d: 'M878 49H894A50 50 0 0 1 944 99V122H852V125A26 26 0 0 0 878 151H894A26 26 0 0 0 908.3 146.71L908.3 172.91A50 50 0 0 1 894 175H878A50 50 0 0 1 828 125V99A50 50 0 0 1 878 49ZM878 73H894A26 26 0 0 1 920 99V98H852V99A26 26 0 0 1 878 73Z', evenodd: true },
];

export function DuoWaveWordmark({ height = 19 }: { height?: number }) {
    return (
        <svg class="duowave-wordmark" height={height} width={(height * 944) / 224} viewBox="0 0 944 224" aria-hidden="true">
            {WORDMARK.map((path) => (
                <path d={path.d} fill="currentColor" fill-rule={path.evenodd ? 'evenodd' : undefined} />
            ))}
        </svg>
    );
}

/** Знак с надписью — в шапке пульта: знак 26 px, надпись 19 px (DESIGN.md). Живой — радуется при наведении на всё. */
export function DuoWaveLogo({ size = 26, name = true, live = false }: { size?: number; name?: boolean; live?: boolean }) {
    const logo = useRef<HTMLSpanElement>(null);
    return (
        <span ref={logo} class="duowave-logo" title="DuoWave" role="img" aria-label="DuoWave">
            {live ? <LiveMark size={size} hover={logo} /> : <DuoWaveMark size={size} />}
            {name && <DuoWaveWordmark height={Math.round((size * 19) / 26)} />}
        </span>
    );
}

/** Внизу экрана: ненавязчиво, монохромно, ссылкой на сайт. */
export function PoweredBy() {
    return (
        <footer class="powered-by">
            <a href="https://duowave.ai" target="_blank" rel="noreferrer">
                <DuoWaveMark size={16} variant="mono" />
                Powered by DuoWave
            </a>
        </footer>
    );
}
