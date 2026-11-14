// MagicPlace по голосу (docs/plans/magicplace.md, «Голос»): шлюз сказал `magicplace.open` — колонку или микрофон
// пульта попросили открыть магазин, поставить приложение или узнать, что обновилось. Пульт открывает окно MagicPlace,
// на пакете — с его окном установки. Как «Примерка» по просьбе колонки (voice/flow.tsx): только слушает вести.

import { useEffect, useRef } from 'preact/hooks';

import { can } from '../api';
import { usePanel } from '../shell/context';

interface Opening {
    item: string | null;
    install: boolean;
}

const listeners = new Set<(opening: Opening) => void>();

/** Весть живого канала: `magicplace.open` — subject — пакет, change — `install` (с окном установки) или `open`. */
export function noteMagicPlaceOpen(name: string, subject?: string | null, change?: string | null): void {
    if (name !== 'magicplace.open') return;
    const opening = { item: subject || null, install: change === 'install' };
    for (const listener of [...listeners]) listener(opening);
}

export function MagicPlaceFlow({ tab }: { tab: string }) {
    const panel = usePanel();
    const latest = useRef(panel);
    latest.current = panel;
    useEffect(() => {
        const open = ({ item, install }: Opening) => {
            if (!can('station.view')) return;
            latest.current.go('magicplace', tab, item ?? undefined, item && install ? { install: '' } : {});
        };
        listeners.add(open);
        return () => {
            listeners.delete(open);
        };
    }, [tab]);
    return null;
}
