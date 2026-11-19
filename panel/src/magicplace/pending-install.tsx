// Персонаж из приглашения, выбранный, пока колонка ещё не центр (проверка начисто 18.11, п. 63). Владелец: «на этом шаге
// жёстко переспросить про демо-ссылку, посмотреть демо-персонажей и уже настроить их; когда колонка дойдёт — накатим».
// Пока центр не готов, ленту читает сам компьютер (его каталог: лица, «Послушать»), а «Поставить» только запоминает
// выбор — здесь, в браузере (не в памяти пульта у шлюза: окно колонки у помощника открыто с того же адреса, via.ts, и
// выбор доживает до него). Колонка стала центром — источник добавляется у центра, персонаж ставится мастером п. 40
// (`auto`: вопросов нет — сразу шаги, свёрнуто, ход на пилюле). Не вышло — выбор остаётся, со словами почему.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, CatalogEntry, installApi, problemOf } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { openInstallWizard } from './wizard-state';

/** Что выбрано и где оно сейчас. */
export interface PendingInstall {
    /** Адрес ленты из приглашения (с `catalog.json`). */
    url: string;
    id: string;
    title: string;
    /** Картинка — адрес у этого компьютера (он читал ленту). */
    image: string | null;
    at: number;
    /** Что идёт, когда центр готов: добавляю набор у центра, читаю его там. */
    step?: 'adding' | 'reading';
    /** Колонка молчит или не прочла набор дольше `READ_LIMIT_MS`: повтор идёт сам, реже, — «поставлю, как ответит». */
    late?: boolean;
    /** Не вышло — словами; выбор при этом остаётся. */
    problem?: string;
}

const KEY = 'install-pending';
/** Центр молчит (поднимается после переноса) — повтор через столько. */
const RETRY_MS = 4000;
/** Столько ждём колонку часто; дальше — реже и со словами «поставлю, как ответит», но сами (замечание 3 теста 19.11). */
const READ_LIMIT_MS = 3 * 60_000;
/** Повтор, когда колонка молчит дольше `READ_LIMIT_MS`. */
const LATE_RETRY_MS = 15_000;

let pending: PendingInstall | null = (() => {
    try {
        const kept = JSON.parse(localStorage.getItem(KEY) ?? 'null');
        return kept && typeof kept === 'object' && typeof kept.id === 'string' && typeof kept.url === 'string' ? (kept as PendingInstall) : null;
    } catch {
        return null;
    }
})();
const listeners = new Set<() => void>();

function keep(next: PendingInstall | null): void {
    pending = next;
    try {
        if (next) localStorage.setItem(KEY, JSON.stringify(next));
        else localStorage.removeItem(KEY);
    } catch {
        // приватное окно: выбор помним до перезагрузки
    }
    for (const listener of [...listeners]) listener();
}

export function pendingInstall(): PendingInstall | null {
    return pending;
}

/** «Поставить», пока колонка не центр: запомнить (один выбор — последний). */
export function choosePending(entry: CatalogEntry, url: string, image: string | null): void {
    keep({ url, id: entry.id, title: entry.title, image, at: Date.now() });
}

/** «Не ставить»: выбор забыт. */
export function dropPending(): void {
    keep(null);
}

function patch(next: Partial<PendingInstall>): void {
    if (pending) keep({ ...pending, ...next });
}

export function usePendingInstall(): PendingInstall | null {
    const [now, setNow] = useState(pending);
    useEffect(() => {
        const listener = () => setNow(pending);
        listeners.add(listener);
        setNow(pending);
        return () => {
            listeners.delete(listener);
        };
    }, []);
    return now;
}

/** Не ответила сама колонка (проход помощника: её нет на связи, она ещё поднимается), а не лента. */
export const centreAway = (e: unknown): boolean =>
    e instanceof TypeError ||
    (e instanceof ApiError && (e.status === 503 || e.status === 504 || (e.status === 502 && /Колонка оборвала|cut the answer/i.test(e.message))));

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** Разбудить ожидание доставки: «Попробовать сейчас». */
let wake: (() => void) | null = null;
/** Пауза доставки, которую «Попробовать сейчас» обрывает. */
const nap = (ms: number) =>
    new Promise<void>((done) => {
        const id = setTimeout(() => ((wake = null), done()), ms);
        wake = () => (clearTimeout(id), (wake = null), done());
    });

/** Добавления, что идут сейчас: та же ссылка у того же центра — один запрос, а не два. */
const adding = new Map<string, Promise<unknown>>();

/**
 * Свой источник у центра (`station` — ключ колонки у помощника; без него — свой шлюз), один запрос на ссылку и центр
 * сразу: приглашение (screens/onb/invite-state.ts) и доставка выбора ниже добавляют одну и ту же ссылку, а колонка
 * сразу после установки отвечает на добавление минуту и дольше — второй такой же запрос она прочла бы набор заново
 * (замечание 3 теста 19.11). Уже добавлен (409) — тоже готово. Очередь дома (202) — правка дойдёт сама.
 */
export async function addSourceAt(station: string | undefined, url: string): Promise<void> {
    const key = `${station ?? 'own'} ${url}`;
    let going = adding.get(key);
    if (!going) {
        going = installApi(station)
            .addPackageSource(url)
            .catch((e: unknown) => {
                if (e instanceof ApiError && e.status === 409 && /уже добавлен|already added/i.test(e.message)) return;
                throw e;
            })
            .finally(() => adding.delete(key));
        adding.set(key, going);
    }
    await going;
}

/** Идёт ли уже доставка (одна на пульт). */
let running = false;

/** Центр готов: источник — у центра, персонаж — мастером. `station` — ключ колонки у помощника; без него — свой шлюз. */
async function land(station: string | undefined): Promise<void> {
    const chosen = pending;
    if (!chosen || running) return;
    running = true;
    const t = tr.onbCharacter;
    const at = installApi(station);
    const started = Date.now();
    const still = () => pending?.id === chosen.id && pending.at === chosen.at;
    // Колонка молчит или долго читает набор — не сдаёмся: дальше реже и со словами, выбор догоняет её сам.
    const late = (): boolean => {
        const over = Date.now() - started > READ_LIMIT_MS;
        if (over && !pending?.late) patch({ late: true });
        return over;
    };
    try {
        patch({ step: 'adding', problem: undefined, late: false });
        for (;;) {
            if (!still()) return;
            try {
                // Очередь дома (202) — правка дойдёт сама; дальше всё равно ждём персонажа в каталоге центра.
                await addSourceAt(station, chosen.url);
                break;
            } catch (e) {
                if (!centreAway(e)) return patch({ step: undefined, late: false, problem: t.pendingAddFailed(problemOf(e)) });
                await nap(late() ? LATE_RETRY_MS : RETRY_MS);
            }
        }
        patch({ step: 'reading' });
        let entry: CatalogEntry | undefined;
        for (;;) {
            if (!still()) return;
            const view = await at.packagesCatalog().catch(() => null);
            entry = view?.packages.find((item) => item.id === chosen.id);
            if (entry) break;
            await nap(late() ? LATE_RETRY_MS : 2500);
        }
        if (!still()) return;
        // Уже стоит у центра (перенос взял его с собой) — ставить нечего.
        keep(null);
        if (entry.installed) return;
        void openInstallWizard(entry, undefined, false, station, true);
    } finally {
        running = false;
    }
}

/**
 * Смотрит, не стал ли центр готов, пока есть отложенный выбор: у помощника — колонка в роли центра (ключ), окно колонки
 * (via) — шлюз сам в роли колонки. Стоит рядом с мастером установки (app.tsx): работает на любом экране, не только на
 * шаге «Персонаж».
 */
export function PendingInstallRunner() {
    const panel = usePanel();
    const chosen = usePendingInstall();
    const helperKey = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const ready = panel.role.role === 'helper' ? !!helperKey : panel.health?.placement?.role === 'station';
    useEffect(() => {
        if (chosen && ready && !chosen.problem) void land(helperKey);
    }, [chosen?.id, chosen?.at, chosen?.problem, ready, helperKey]);
    return null;
}

/** «Ещё раз» после отказа — снова к центру; «Попробовать сейчас», пока ждём колонку, — не дожидаясь паузы. */
export function retryPending(): void {
    if (pending?.late && !pending.problem) return void wake?.();
    patch({ problem: undefined, step: undefined, late: false });
}
