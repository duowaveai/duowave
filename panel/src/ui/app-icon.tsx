// Значок приложения — на плитке «Моих» и на ярлыке стола (docs/plans/menu-scheme.md): картинка приложения, если она
// есть; нет — цветная плитка с его значком из пульта (значок раздела или виджета), а без значка — с первой буквой.
// Цвет — один из трёх градиентов бренда (тёплый, холодный, фиолетовый), по id: у приложения он не меняется.

import { IconMagicPlace, MagicPlaceMark } from '../magicplace/icon';
import { registeredPlugins } from '../shell/registry';
import type { Icon } from '../shell/sections';

const TONES = ['warm', 'cool', 'violet'] as const;

function toneOf(id: string): (typeof TONES)[number] {
    let hash = 0;
    for (const char of id) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    return TONES[hash % TONES.length];
}

/** Значок приложения в пульте: раздела (окна), иначе первого виджета стола. */
export function appIconOf(id: string): Icon | undefined {
    const plugin = registeredPlugins().find((entry) => entry.id === id || entry.id === `package.${id}`);
    return plugin?.sections?.[0]?.icon ?? plugin?.slots?.['desk.widgets']?.[0]?.icon;
}

export function AppIcon({ id, title, image, icon, size = 44 }: { id: string; title: string; image?: string | null; icon?: Icon; size?: number }) {
    // MagicPlace — встроенный магазин, не пакет: его знак всегда свой, розовый.
    if (id === 'magicplace' || icon === IconMagicPlace) return <MagicPlaceMark size={size} />;
    const style = { width: `${size}px`, height: `${size}px` };
    if (image) return <img class="app-icon is-image" src={image} alt="" style={style} />;
    const Glyph = icon ?? appIconOf(id);
    return (
        <span class={`app-icon is-${toneOf(id)}`} style={style} aria-hidden="true">
            {Glyph ? <Glyph size={Math.round(size * 0.46)} /> : <span class="app-icon-letter">{title.trim().slice(0, 1).toUpperCase()}</span>}
        </span>
    );
}
