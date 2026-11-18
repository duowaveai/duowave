// Приглашение первых шагов — одно на все шаги (проверка начисто 18.11, п. 64). Было состоянием компонента на шаге
// «Персонаж»: ушёл к «Колонке» — ввод и ход добавления пропадали; колонка стала центром — шаг пересоздавался с её
// ключом, у центра своих источников нет — и снова «Вставьте ссылку». Теперь решение и работа живут здесь: что решено,
// какая ссылка, где набор уже прочитан; ведёт добавление и чтение сам, шаг только рисует. Помнится во вкладке
// (sessionStorage): переживает уход со шага, перезапуск шлюза и перезагрузку окна.
//
// Где читать набор («дом»): у центра (ключ колонки у помощника), а пока колонка не центр — у этого компьютера (п. 63:
// Mac сам читает ленту). Дом сменился (колонка стала центром) — набор добавляется и у нового дома, без нового ввода.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, CatalogView, installApi, problemOf } from '../../api';
import { tr } from '../../i18n';
import { centreAway } from '../../magicplace/pending-install';
import { recalled, remember } from '../../shell/memory';

/**
 * - none — ещё не решили: «Дальше» ведёт к карточке;
 * - skipped — «Пропустить»;
 * - accepted — ссылку приняли, работа вот-вот начнётся;
 * - waiting — ссылку приняли, дом не отвечает (колонка поднимается): догружу сам, «Дальше» свободна;
 * - adding / reading — добавляю набор / читаю персонажей: «Дальше» крутится (п. 60);
 * - shown — персонажи показаны;
 * - failed — не вышло, словами; ссылка в поле, «Дальше» свободна.
 */
export type InviteStage = 'none' | 'skipped' | 'accepted' | 'waiting' | 'adding' | 'reading' | 'shown' | 'failed';

export interface InviteState {
    stage: InviteStage;
    /** Принятая ссылка — адрес ленты (с `catalog.json`). */
    url?: string;
    /** Где набор прочитан и показан: ключ колонки или 'own'. */
    read?: string;
    /** Сколько персонажей показано. */
    count?: number;
    /** Что не так — словами. */
    problem?: string;
    /** Что вписано в поле, ещё не принято. */
    draft: string;
    /** Ссылка разобрана, ждёт «Понятно, добавить». */
    asking?: string;
}

/** «Пропустить» — и в памяти пульта: дальше приглашение — свёрнутой строкой (п. 53). */
export const SKIP_KEY = 'onb-invite-skipped';
const KEY = 'onb-invite';
/** Дом молчит — повтор через столько. */
const RETRY_MS = 4000;
/** Лента ещё читается: перечитываем так часто. */
const READ_EVERY_MS = 2000;
/** Добавление и чтение — не дольше (п. 60), дальше «не загрузился». Ожидание дома сюда не входит. */
const LOAD_LIMIT_MS = 60_000;

let state: InviteState = (() => {
    try {
        const kept = JSON.parse(sessionStorage.getItem(KEY) ?? 'null');
        if (kept && typeof kept === 'object' && typeof kept.stage === 'string') return { draft: '', ...kept } as InviteState;
    } catch {
        // приватное окно — начнём с начала
    }
    return { stage: recalled<boolean>(SKIP_KEY) === true ? 'skipped' : 'none', draft: '' };
})();
const listeners = new Set<() => void>();
/** Дом сейчас — его сообщают первые шаги (steerInvite). */
let home: string | null = null;
/** Работа идёт под этим номером; новый — прежняя тихо кончается. */
let generation = 0;
/** Что делается сейчас (ссылка и дом) — тот же вызов не начинает работу заново. */
let active: string | null = null;

function set(next: Partial<InviteState>): void {
    state = { ...state, ...next };
    try {
        sessionStorage.setItem(KEY, JSON.stringify(state));
    } catch {
        // помним до перезагрузки
    }
    for (const listener of [...listeners]) listener();
}

export function inviteState(): InviteState {
    return state;
}

export function useInvite(): InviteState {
    const [now, setNow] = useState(state);
    useEffect(() => {
        const listener = () => setNow(state);
        listeners.add(listener);
        setNow(state);
        return () => {
            listeners.delete(listener);
        };
    }, []);
    return now;
}

export function setDraft(draft: string): void {
    set({ draft });
}

/** Ссылка разобрана — спросить про непроверенные голоса; null — передумали. */
export function askInvite(url: string | null, problem?: string): void {
    set({ asking: url ?? undefined, problem });
}

/** Сказать, что не так со ссылкой, не принимая её. */
export function inviteProblem(problem: string | undefined): void {
    set({ problem });
}

/** «Понятно, добавить»: ссылка принята, дальше — сами. */
export function acceptInvite(url: string): void {
    generation++;
    active = null;
    set({ stage: 'accepted', url, asking: undefined, problem: undefined, draft: '', read: undefined, count: undefined });
    remember(SKIP_KEY, undefined);
    kick();
}

/** «Отмена» посреди добавления: ссылка возвращается в поле. */
export function cancelInvite(): void {
    generation++;
    active = null;
    set({ stage: 'none', draft: state.url ?? state.draft, url: undefined, asking: undefined, problem: undefined, read: undefined });
}

export function skipInvite(): void {
    generation++;
    active = null;
    remember(SKIP_KEY, true);
    set({ stage: 'skipped', asking: undefined, problem: undefined });
}

/** Первые шаги говорят, где сейчас дом (ключ колонки у помощника или 'own'); сменился — догружаем у нового. */
export function steerInvite(next: string): void {
    if (home === next) return void kick();
    home = next;
    generation++;
    active = null;
    kick();
}

/** Что-то надо сделать: ссылка принята, а у этого дома набор ещё не показан — работаем. */
function kick(): void {
    if (!home || !state.url) return;
    if (state.stage === 'none' || state.stage === 'skipped' || state.stage === 'failed') return;
    if (state.stage === 'shown' && state.read === home) return;
    const doing = `${state.url} @ ${home}`;
    if (active === doing) return;
    const at = ++generation;
    active = doing;
    void work(at, state.url, home).finally(() => {
        if (at === generation) active = null;
    });
}

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** Персонажи своих источников, которых ещё нет здесь; скрытые лентой не показываются. */
export function demosCount(view: CatalogView | null): number {
    return (view?.packages ?? []).filter((entry) => entry.own && entry.kind === 'character' && entry.state === 'available' && !entry.unlisted).length;
}

/** Ошибка добавления — простыми словами: не ссылка, не открылась, не лента, источников уже четыре. */
export function addProblem(e: unknown): string {
    const t = tr.onbCharacter;
    if (!(e instanceof ApiError)) return problemOf(e);
    if (e.status === 400) return t.inviteNotLink;
    if (e.status === 502) return /HTTP \d{3}|не отвечает|not respond|unreachable|timed? ?out/i.test(e.message) ? t.inviteUnreachable : t.inviteNotFeed;
    if (e.status === 409 && /не больше|no more/i.test(e.message)) return t.inviteFull;
    return problemOf(e);
}

/** Добавить набор у дома и дочитать его: добавлен (или уже был) → лента прочитана → показаны. */
async function work(at: number, url: string, where: string): Promise<void> {
    const api = installApi(where === 'own' ? undefined : where);
    const alive = () => at === generation && state.url === url;
    let since = Date.now();
    set({ stage: 'adding', problem: undefined });
    // Дом не отвечает (колонка поднимается, шлюз перезапускается) или правка в очереди дома — ждём, не повторяя ввод.
    let queued = false;
    for (;;) {
        if (!alive()) return;
        try {
            const added = await api.addPackageSource(url);
            queued = 'queued' in added;
            break;
        } catch (e) {
            if (!alive()) return;
            if (e instanceof ApiError && e.status === 409 && /уже добавлен|already added/i.test(e.message)) break;
            if (!centreAway(e)) return set({ stage: 'failed', problem: addProblem(e), draft: url });
            set({ stage: 'waiting' });
            await sleep(RETRY_MS);
            since = Date.now();
            if (alive()) set({ stage: 'adding' });
        }
    }
    if (queued) set({ stage: 'waiting' });
    else set({ stage: 'reading' });
    // Лента у дома: читается сама, со своими превью (п. 59) — перечитываем, пока не прочитана.
    for (;;) {
        if (!alive()) return;
        const view = await api.packagesCatalog().catch(() => null);
        if (!alive()) return;
        const feed = view?.feeds?.find((entry) => entry.own && entry.url === url);
        if (!view || (queued && !feed)) {
            // Дом молчит или правка ещё в очереди: ждём, «Дальше» свободна.
            if (state.stage !== 'waiting') set({ stage: 'waiting' });
            await sleep(RETRY_MS);
            since = Date.now();
            continue;
        }
        if (state.stage === 'waiting') set({ stage: 'reading' });
        // Готово — когда шлюз прочитал именно эту ленту; её нет в списке (шлюз постарше) — по персонажам.
        const ready = feed ? !!feed.fetchedAt : demosCount(view) > 0;
        if (ready) return set({ stage: 'shown', read: where, count: demosCount(view), problem: undefined });
        if (Date.now() - since > LOAD_LIMIT_MS) return set({ stage: 'failed', problem: tr.onbCharacter.inviteTimeout, draft: url });
        await sleep(READ_EVERY_MS);
    }
}
