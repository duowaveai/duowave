// Вид карточки (`look`, gateway/src/bus/cards.rs): цвета фона градиентом, своя картинка фоном и живая сцена пульта —
// дождь, снег, солнце, звёзды, волны (scenes.ts). Красит весь виджет вместе с шапкой: рамка стола даёт карточке
// контекст `WidgetPaint`; где рамки нет (черновик, чат), карточка красит свою коробку сама.

import { createContext } from 'preact';
import { useContext, useEffect, useRef, useState } from 'preact/hooks';

import { fetchBusPreview, fetchPluginFile } from '../api';
import { runScene, SCENE_COLORS, SCENES, SceneName } from './scenes';

export interface CardLook {
    scene?: SceneName;
    colors?: string[];
    picture?: { name?: string; address?: string };
    /** Цвет текста: light — светлый (по умолчанию), dark — тёмный на светлом фоне. */
    ink?: 'light' | 'dark';
}

/** Вид, которым пульт умеет красить: незнакомое отброшено, пусто — null. */
export function lookOf(raw: unknown): CardLook | null {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
    const given = raw as Record<string, unknown>;
    const look: CardLook = {};
    if (typeof given.scene === 'string' && (SCENES as string[]).includes(given.scene)) look.scene = given.scene as SceneName;
    if (Array.isArray(given.colors)) {
        const colors = given.colors.filter((color): color is string => typeof color === 'string' && /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(color)).slice(0, 3);
        if (colors.length > 0) look.colors = colors;
    }
    const picture = given.picture as { name?: unknown; address?: unknown } | undefined;
    if (picture && typeof picture === 'object') {
        if (typeof picture.name === 'string' && picture.name) look.picture = { name: picture.name };
        else if (typeof picture.address === 'string' && picture.address.startsWith('files/')) look.picture = { address: picture.address };
    }
    if (given.ink === 'dark' || given.ink === 'light') look.ink = given.ink;
    return look.scene || look.colors || look.picture ? look : null;
}

/** Цвета фона вида: свои, иначе цвета сцены, иначе спокойный тёмный. */
function colorsOf(look: CardLook): string[] {
    return look.colors ?? (look.scene ? SCENE_COLORS[look.scene] : ['#2b2f3a', '#454b59']);
}

/** Фон вида градиентом. */
export function gradientOf(look: CardLook): string {
    const colors = colorsOf(look);
    return colors.length === 1 ? colors[0] : `linear-gradient(160deg, ${colors.join(', ')})`;
}

/** Яркость цвета для глаза (WCAG, 0 — чёрный, 1 — белый). */
function luminance(color: string): number {
    let hex = color.replace('#', '');
    if (hex.length === 3) hex = hex.replace(/./g, (c) => c + c);
    const channel = (at: number): number => {
        const value = parseInt(hex.slice(at, at + 2), 16) / 255;
        return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}

/**
 * Каким текстом писать по фону и надо ли его притушить. Цвет текста — свой у вида или по яркости фона: светлый фон —
 * тёмный текст. Край градиента, на котором текст не прочтётся (белый текст на светлом краю, тёмный на тёмном),
 * пульт притушивает тенью: вид не зависит от темы пульта и всегда читается.
 */
export function inkOf(look: CardLook): { ink: 'light' | 'dark'; shade: 'soft' | 'strong' } {
    const levels = colorsOf(look).map(luminance);
    const average = levels.reduce((sum, level) => sum + level, 0) / levels.length;
    const ink = look.ink ?? (average > 0.45 && !look.picture ? 'dark' : 'light');
    const clash = ink === 'light' ? Math.max(...levels) > 0.55 : Math.min(...levels) < 0.18;
    return { ink, shade: clash ? 'strong' : 'soft' };
}

/** Стабильный случай сцены по приложению: у погоды одни и те же облака после перерисовки. */
function seedOf(text: string): number {
    let hash = 2166136261;
    for (let i = 0; i < text.length; i += 1) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619);
    return hash >>> 0;
}

function SceneCanvas({ scene, seed }: { scene: SceneName; seed: number }) {
    const canvas = useRef<HTMLCanvasElement>(null);
    useEffect(() => {
        if (!canvas.current) return undefined;
        return runScene(canvas.current, scene, seed);
    }, [scene, seed]);
    return <canvas ref={canvas} class="look-scene" />;
}

function Backdrop({ app, picture }: { app: string; picture: NonNullable<CardLook['picture']> }) {
    const [url, setUrl] = useState<string | null>(null);
    useEffect(() => {
        let made: string | null = null;
        let alive = true;
        (picture.address ? fetchBusPreview(app, picture.address, 960) : fetchPluginFile(app, picture.name ?? ''))
            .then((blob) => {
                if (!alive) return;
                made = URL.createObjectURL(blob);
                setUrl(made);
            })
            .catch(() => alive && setUrl(null));
        return () => {
            alive = false;
            if (made) URL.revokeObjectURL(made);
        };
    }, [app, picture.name, picture.address]);
    return url ? <img class="look-picture" src={url} alt="" /> : null;
}

/** Слои вида за содержимым: градиент, фото, сцена, тень для текста. */
export function LookLayers({ app, look }: { app: string; look: CardLook }) {
    return (
        <div class="look-layers" aria-hidden="true" style={{ background: gradientOf(look) }}>
            {look.picture && <Backdrop app={app} picture={look.picture} />}
            {look.scene && <SceneCanvas scene={look.scene} seed={seedOf(`${app}:${look.scene}`)} />}
            <div class="look-shade" data-ink={inkOf(look).ink} data-shade={inkOf(look).shade} />
        </div>
    );
}

/** Рамка стола даёт карточке, чем покрасить весь виджет; null — рамки нет, карточка красит себя сама. */
export const WidgetPaint = createContext<((paint: { app: string; look: CardLook } | null) => void) | null>(null);

/**
 * Вид карточки отдан рамке виджета, если она есть: вернёт true — красит рамка; false — пусть карточка красит себя.
 */
export function usePaint(app: string, look: CardLook | null): boolean {
    const paint = useContext(WidgetPaint);
    const key = look ? JSON.stringify(look) : '';
    useEffect(() => {
        if (!paint) return undefined;
        paint(look ? { app, look } : null);
        return () => paint(null);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [paint, app, key]);
    return Boolean(paint);
}
