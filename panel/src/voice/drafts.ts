// Черновики на столе (docs/plans/voice-creation.md, «Плитка-черновик»): что делает агент мастерской и что ждёт «на
// примерке». Шлюз говорит о каждой перемене черновика событием `workshop.changed` — с его id и видом перемены (`made`,
// `ready`, `failed`, `parked`, `installed`, `open:fitting`…). Стол показывает одну плитку — самый свежий черновик
// (решение 9), остальные — «и ещё N» в плашке и в мастерской. Колонка просит открыть «Примерку» (`open:fitting`) или
// «да» (`open:consent`) — пульт открывает окно.

import { useEffect, useRef, useState } from 'preact/hooks';

import { DraftStage, DraftView, WorkshopAgentModel, api, can } from '../api';
import { usePoll } from '../poll';

type Listener = (draft: string, change: string) => void;

const listeners = new Set<Listener>();

/** Перемена мастерской с шлюза (живой канал): чей черновик и что с ним. Без черновика — перечитать всё. */
export function noteWorkshopChange(draft?: string, change?: string): void {
    for (const listener of [...listeners]) listener(draft ?? '', change ?? '');
}

/** Слушать перемены мастерской. Возвращает, как перестать. */
export function onWorkshopChange(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Стадии, в которых черновик стоит на столе плиткой: агент работает, «на примерке», не вышло, остановлен. */
export const ON_DESK: DraftStage[] = ['working', 'ready', 'failed', 'stopped'];

/** Черновик плиткой на столе — самый свежий из тех, что на столе; остальные считаются. */
export function deskDrafts(drafts: DraftView[]): { tile: DraftView | null; more: number } {
    const shown = drafts.filter((draft) => ON_DESK.includes(draft.meta.stage)).sort((a, b) => b.meta.updated - a.meta.updated);
    return { tile: shown[0] ?? null, more: Math.max(0, shown.length - 1) };
}

export interface DeskDrafts {
    /** Плитка на столе: самый свежий черновик в работе или на примерке. */
    tile: DraftView | null;
    /** Сколько ещё таких — «и ещё N». */
    more: number;
    /** Перечитать сейчас (после своего действия). */
    reload: () => void;
}

/** Пока агент работает — перечитывать так часто, даже если живой канал молчит. */
const WORKING_MS = 3000;

/**
 * Черновики для стола: сразу, по каждой перемене мастерской и, пока агент работает, раз в три секунды. Только у того,
 * кто может мастерскую (plugins.manage): у остальных плитки нет.
 */
export function useDeskDrafts(): DeskDrafts {
    const allowed = can('plugins.manage');
    const [drafts, setDrafts] = useState<DraftView[]>([]);
    const [version, setVersion] = useState(0);
    useEffect(() => onWorkshopChange((_, change) => change !== 'agent' && setVersion((n) => n + 1)), []);
    const working = drafts.some((draft) => draft.meta.stage === 'working');
    const reload = usePoll({
        on: allowed,
        every: working ? WORKING_MS : 0,
        version,
        settle: 300,
        read: (signal) =>
            api.workshop(signal).then((view) => {
                if (!signal.aborted) setDrafts(view.drafts);
            }),
    });
    const { tile, more } = deskDrafts(drafts);
    return { tile, more, reload };
}

/** Где работает агент мастерской: модель в облаке (янтарь) или дома. null — не знаем. */
export function useAgentPlace(): boolean | null {
    const [model, setModel] = useState<WorkshopAgentModel | null>(null);
    const asked = useRef(false);
    useEffect(() => {
        if (asked.current || !can('plugins.manage')) return;
        asked.current = true;
        api.workshopAgent()
            .then(setModel)
            .catch(() => undefined);
    }, []);
    return model?.answers ? model.answers.cloud : null;
}
