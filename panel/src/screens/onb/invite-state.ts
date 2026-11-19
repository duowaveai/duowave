// Приглашение первых шагов — одно на все шаги (проверка начисто 18.11, п. 64). Было состоянием компонента на шаге
// «Персонаж»: ушёл к «Колонке» — ввод и ход добавления пропадали; колонка стала центром — шаг пересоздавался с её
// ключом, у центра своих источников нет — и снова «Вставьте ссылку». Теперь решение и работа живут здесь: что решено,
// какая ссылка, где набор уже прочитан; ведёт добавление и чтение сам, шаг только рисует. Помнится во вкладке
// (sessionStorage): переживает уход со шага, перезапуск шлюза и перезагрузку окна.
//
// Где читать набор («дом»): у этого компьютера — он читает ленту за секунду (п. 63), персонажи видны сразу. Центр —
// колонка (ключ колонки у помощника) — получает набор в фоне: «колонка догонит» (замечание 3 теста 19.11: колонка
// сразу после установки собирала набор минуту, и человек ждал её). Выбранного персонажа ей доносит
// magicplace/pending-install.tsx; набор добавляется у центра одним запросом (`addSourceAt`), повторная вставка дублей
// не даёт.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, CatalogView, installApi, problemOf, stationRead } from '../../api';
import { tr } from '../../i18n';
import { addSourceAt, centreAway } from '../../magicplace/pending-install';
import { recalled, remember } from '../../shell/memory';
import { patchRecord, readRecord } from './record';

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

/**
 * Набор у центра-колонки (у помощника), после того как этот компьютер его прочитал:
 * - sending — добавляю у колонки (она может отвечать минуту);
 * - away — колонка долго не отвечает, добавлю, как ответит (повтор идёт);
 * - sent — добавлен у колонки или ждёт её в очереди дома;
 * - failed — колонка отказала, словами; «Ещё раз».
 */
export type CentreStage = 'sending' | 'away' | 'sent' | 'failed';

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
    /** Набор у центра-колонки; нет — центра нет (этот компьютер сам центр) или ещё не начинали. */
    centre?: CentreStage;
    /** Какой колонке относится `centre`. */
    centreAt?: string;
    /** Колонка отказала — словами. */
    centreProblem?: string;
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
/** Колонка молчит дольше — строка «добавлю, как ответит» вместо «добавляю». */
const CENTRE_AWAY_MS = 3 * 60_000;

/** Сохранённое приглашение: из записи первых шагов (onb/record.ts — переживает перезапуск приложения), иначе из вкладки. */
function kept(): InviteState | null {
    const valid = (value: unknown): value is InviteState =>
        !!value && typeof value === 'object' && typeof (value as InviteState).stage === 'string';
    const recorded = readRecord()?.invite;
    if (valid(recorded)) return { ...recorded, draft: recorded.draft ?? '' };
    try {
        const session = JSON.parse(sessionStorage.getItem(KEY) ?? 'null');
        if (valid(session)) return { ...session, draft: session.draft ?? '' };
    } catch {
        // приватное окно — начнём с начала
    }
    return null;
}

let state: InviteState = kept() ?? { stage: recalled<boolean>(SKIP_KEY) === true ? 'skipped' : 'none', draft: '' };
const listeners = new Set<() => void>();
/** Дом сейчас — его сообщают первые шаги (steerInvite). */
let home: string | null = null;
/** Центр-колонка у помощника (ключ) — ему набор в фоне; null — центра отдельно нет. */
let centre: string | null = null;
/** Набор уходит к этому центру сейчас (ссылка и ключ): тот же не начинаем заново. */
let delivering: string | null = null;
/** Работа идёт под этим номером; новый — прежняя тихо кончается. */
let generation = 0;
/** Что делается сейчас (ссылка и дом) — тот же вызов не начинает работу заново. */
let active: string | null = null;

function set(next: Partial<InviteState>): void {
    state = { ...state, ...next };
    // В запись первых шагов, пока они открыты (замечание 10 теста 19.11): ссылка, ход и доставка колонке переживают
    // перезапуск приложения и шлюза. Шаги закрыты — только во вкладке, как прежде.
    patchRecord({ invite: state });
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
    set({
        stage: 'accepted',
        url,
        asking: undefined,
        problem: undefined,
        draft: '',
        read: undefined,
        count: undefined,
        centre: undefined,
        centreProblem: undefined,
    });
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

/**
 * Первые шаги говорят, где читать набор (`next`: 'own' — этот компьютер) и есть ли отдельный центр (`toCentre` — ключ
 * колонки у помощника): ему набор уходит в фоне. Дом сменился — догружаем у нового.
 */
export function steerInvite(next: string, toCentre: string | null = null): void {
    // Память пульта пришла со шлюза позже, чем модуль прочитал её копию (копию в браузере стёрли): приглашение — из записи.
    if (state.stage === 'none' && !state.url) {
        const restored = kept();
        if (restored && restored.stage !== 'none') state = restored;
    }
    centre = toCentre;
    if (home === next) return void kick();
    home = next;
    generation++;
    active = null;
    kick();
}

/** «Ещё раз» после отказа колонки. */
export function retryCentre(): void {
    set({ centre: undefined, centreProblem: undefined });
    kickCentre();
}

/** Что-то надо сделать: ссылка принята, а у этого дома набор ещё не показан — работаем. */
function kick(): void {
    if (!home || !state.url) return;
    if (state.stage === 'none' || state.stage === 'skipped' || state.stage === 'failed') return;
    if (state.stage === 'shown' && state.read === home) return void kickCentre();
    const doing = `${state.url} @ ${home}`;
    if (active === doing) return;
    const at = ++generation;
    active = doing;
    void work(at, state.url, home).finally(() => {
        if (at === generation) active = null;
    });
}

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

/**
 * Набор прочитан здесь — пора к центру-колонке: в фоне, пока не дойдёт; «Дальше» не ждёт. Ссылка уже проверена этим
 * компьютером (лента читается, подпись и формат — те же), так что колонке уходит только годная.
 */
function kickCentre(): void {
    const url = state.url;
    if (!centre || !url || state.stage !== 'shown') return;
    if (state.centreAt === centre && (state.centre === 'sent' || state.centre === 'failed')) return;
    const doing = `${url} @ ${centre}`;
    if (delivering === doing) return;
    delivering = doing;
    void deliver(url, centre).finally(() => {
        if (delivering === doing) delivering = null;
    });
}

async function deliver(url: string, to: string): Promise<void> {
    // Отменили, пропустили, вставили другую ссылку или центр сменился — дальше не несём.
    const alive = () => state.url === url && centre === to && state.stage === 'shown';
    const started = Date.now();
    set({ centre: 'sending', centreAt: to, centreProblem: undefined });
    for (;;) {
        if (!alive()) return;
        try {
            await addSourceAt(to, url);
            if (alive()) set({ centre: 'sent' });
            return;
        } catch (e) {
            if (!alive()) return;
            if (!centreAway(e)) return set({ centre: 'failed', centreProblem: addProblem(e) });
            const away = Date.now() - started > CENTRE_AWAY_MS;
            if (state.centre !== (away ? 'away' : 'sending')) set({ centre: away ? 'away' : 'sending' });
            await sleep(RETRY_MS);
        }
    }
}

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

/**
 * Каталог дома — живой ответ, а не копия. Колонка не на связи (поднимается, ставится) — проход помощника отдаёт
 * последнюю копию (`x-duowave-copy-at`), снятую до добавления набора: в ней нет ни ленты, ни её персонажей, и она
 * не ответ — тогда null, «центр пока не отвечает», ждём (замечание 2 теста 19.11: копия минуту шла за ответ, и
 * выходило «Набор не загрузился»).
 */
async function catalogOf(api: ReturnType<typeof installApi>, where: string): Promise<CatalogView | null> {
    if (where === 'own') return api.packagesCatalog().catch(() => null);
    const answer = await stationRead<CatalogView>(where, '/api/v1/packages/catalog').catch(() => null);
    return answer && answer.copyAt === null ? answer.value : null;
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
    // Минута чтения — от ответа на добавление: колонка сразу после установки отвечает на него минуту и дольше (она
    // в нём же читает ленту и превью), и прежде к первому чтению минута уже выходила (замечание 2 теста 19.11).
    since = Date.now();
    // Лента у дома: читается сама, со своими превью (п. 59) — перечитываем, пока не прочитана.
    for (;;) {
        if (!alive()) return;
        const view = await catalogOf(api, where);
        if (!alive()) return;
        const feed = view?.feeds?.find((entry) => entry.own && entry.url === url);
        if (!view || (queued && !feed)) {
            // Дом молчит (или отдана копия) или правка ещё в очереди: ждём, «Дальше» свободна.
            if (state.stage !== 'waiting') set({ stage: 'waiting' });
            await sleep(RETRY_MS);
            since = Date.now();
            continue;
        }
        if (state.stage === 'waiting') set({ stage: 'reading' });
        // Готово — когда шлюз прочитал именно эту ленту; её нет в списке (шлюз постарше) — по персонажам.
        const ready = feed ? !!feed.fetchedAt : demosCount(view) > 0;
        if (ready) {
            set({ stage: 'shown', read: where, count: demosCount(view), problem: undefined });
            return kickCentre();
        }
        if (Date.now() - since > LOAD_LIMIT_MS) return set({ stage: 'failed', problem: tr.onbCharacter.inviteTimeout, draft: url });
        await sleep(READ_EVERY_MS);
    }
}
