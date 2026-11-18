// Починка раскладки стола без DOM (проверка начисто 18.11, п. 47): отдельно от shell/widgets.ts, чтобы её проверял
// panel-check.

import type { DeskLayout, DeskWidget } from './widgets';

/**
 * Плитка пакета, которую стол рисует, вместо ключа, которого у него нет (проверка начисто 18.11, п. 47): «На стол»
 * голосом и кнопкой клал `package.<пакет>.main`, пока примерка не знала плитку, а карточка приложения на столе —
 * `package.<пакет>.card.main`. Запись такого ключа (поставлен, новый, место, порядок, убран) переходит на первую плитку
 * пакета, когда её ключ не записан сам. Ничего не поменялось — null.
 */
export function repairedLayout(layout: DeskLayout, widgets: Pick<DeskWidget, 'key'>[]): DeskLayout | null {
    const known = new Set(widgets.map((widget) => widget.key));
    const used = new Set([...layout.added, ...layout.hidden, ...Object.keys(layout.places), ...layout.order, ...Object.keys(layout.fresh ?? {})]);
    const moves = new Map<string, string>();
    for (const key of used) {
        const match = /^package\.([^.]+)\.main$/.exec(key);
        if (!match || known.has(key)) continue;
        const real = widgets.find((widget) => widget.key.startsWith(`package.${match[1]}.`));
        if (real && !used.has(real.key)) moves.set(key, real.key);
    }
    if (moves.size === 0) return null;
    const to = (key: string): string => moves.get(key) ?? key;
    const rename = <T,>(record: Record<string, T> | undefined): Record<string, T> | undefined =>
        record && Object.fromEntries(Object.entries(record).map(([key, value]) => [to(key), value]));
    return {
        ...layout,
        added: layout.added.map(to),
        hidden: layout.hidden.map(to),
        order: layout.order.map(to),
        places: rename(layout.places) ?? {},
        fresh: rename(layout.fresh),
    };
}

