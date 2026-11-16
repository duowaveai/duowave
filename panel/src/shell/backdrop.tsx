// Рисованный фон темы за столом (THEMES.md, «Фон»): градиент и до трёх живых сцен пульта (cards/scenes.ts) — звёзды,
// сияние, снег… Тема только называет слои, рисует пульт: тема — данные, кода в ней нет. Фон стоит под всем пультом
// (z-index −1), стол над ним прозрачный (`html[data-backdrop]`, ui/desk.css); карточки и окна — свои, непрозрачные.
// Сцены рисуют светлым; на светлом фоне холст переворачивается в тёмное с тем же оттенком.

import { useEffect, useRef } from 'preact/hooks';

import { runScene, SceneName } from '../cards/scenes';
import { useTheme } from './theme';
import { isLight, luminance } from './theme-colors';
import type { Backdrop } from './themes';

function Layer({ scene, opacity, seed }: { scene: SceneName; opacity: number; seed: number }) {
    const canvas = useRef<HTMLCanvasElement>(null);
    useEffect(() => {
        if (!canvas.current) return undefined;
        return runScene(canvas.current, scene, seed, 1);
    }, [scene, seed]);
    return <canvas ref={canvas} class="theme-backdrop-layer" style={{ opacity }} />;
}

/** Фон градиентом из своих цветов, а без них — ровный `bg` режима. */
function paintOf(backdrop: Backdrop, bg: string): { background: string; light: boolean } {
    const colors = backdrop.colors ?? [bg];
    const average = colors.reduce((sum, color) => sum + luminance(color), 0) / colors.length;
    const background = colors.length === 1 ? colors[0] : `linear-gradient(160deg, ${colors.join(', ')})`;
    return { background, light: colors.length === 1 ? isLight(colors[0]) : average > 0.35 };
}

/** Фон выбранной темы в нынешнем режиме; у темы без фона — ничего. */
export function ThemeBackdrop() {
    const { theme, mode } = useTheme();
    const backdrop = theme.backdrop?.[mode];
    if (!backdrop) return null;
    const bg = theme.palette[mode].bg ?? '#000000';
    const { background, light } = paintOf(backdrop, /^#[0-9a-f]{3,6}$/i.test(bg) ? bg : '#000000');
    return (
        <div class="theme-backdrop" aria-hidden="true" data-ink={light ? 'dark' : 'light'} style={{ background }}>
            {backdrop.layers.map((layer, index) => (
                <Layer key={`${theme.id}:${layer.scene}:${index}`} scene={layer.scene} opacity={layer.opacity} seed={17 + index * 101} />
            ))}
            {backdrop.dim > 0 && <div class="theme-backdrop-dim" style={{ background: bg, opacity: backdrop.dim }} />}
        </div>
    );
}
