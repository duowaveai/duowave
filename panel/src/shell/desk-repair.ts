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


/**
 * Виджеты, что поставили голосом мимо «Примерки» (проверка начисто 19.11, «Fresh Hip-Hop & Rap»): «да» или «на стол» в
 * ответ на «Готово. На стол или примерить?» ставит приложение на шлюзе (talk/workshop.rs, `End::Desk`), и шлюз говорит
 * «На столе.», но раскладку стола держит пульт — плитка на стол не садилась, а голос считал её там. Черновик любого
 * вида — виджет, игра, приложение (замечание 6 теста 19.11), — уже поставленный (`installed`), чьей плитки нет ни среди
 * поставленных, ни среди убранных, садится на стол — один раз: убранный человеком не возвращается. Только плитка,
 * которую стол уже рисует: пока часть приложения не загрузилась, ждать её, а не класть ключ, которого стол не знает.
 */
export function landingsOf(installed: string[], layout: Pick<DeskLayout, 'added' | 'hidden'>, widgets: Pick<DeskWidget, 'key'>[]): string[] {
    const keys: string[] = [];
    for (const id of installed) {
        const own = (key: string): boolean => key.startsWith(`package.${id}.`);
        if (layout.added.some(own) || layout.hidden.some(own)) continue;
        const tile = widgets.find((widget) => own(widget.key));
        if (tile && !keys.includes(tile.key)) keys.push(tile.key);
    }
    return keys;
}
