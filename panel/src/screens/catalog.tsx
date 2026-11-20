// Пакеты и общий каталог: «Приложения» → «Установлено» показывает, что стоит (свои — первыми, с фильтром по
// происхождению); сам каталог — магазин MagicPlace (magicplace/*, docs/plans/magicplace.md), «Персонаж» → «Персонажи» —
// он же, отфильтрованный по виду character. Хуки, метки, окно пакета и «Свои источники» — отсюда, общие для них.
// Каталог сам ничего не ставит и не обновляет: только кнопка человека с правом на этот вид (шлюз всё равно проверяет
// каждое действие сам).

import { OtherDevicesApps } from './home-apps';
import { useEffect, useState } from 'preact/hooks';

import { ApiError, CatalogEntry, CatalogSource, CatalogView, HomeVoicePlan, PackageKind, PackageOrigin, PackageView, PackagesView, VoiceLabEngine, api, can, installApi, problemOf, sessionStore } from '../api';
import { IconPackage } from '../icons';
import { busChanged } from '../bus/state';
import { openInstallWizard, unfinishedInstall } from '../magicplace/wizard-state';
import { tr } from '../i18n';
import { useLinkedItem, usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { AppIcon } from '../ui/app-icon';
import { Action, CardNote } from '../ui/cards';
import { PlayButton, useMediaUrl } from '../ui/media';
import { needsSize } from '../ui/needs';
import { defaultEngine, planGb, usableEngines } from '../ui/voice-plan';
import { InstalledPackageSheet, LaterLine, SchedulesSwitchLine } from './plugins';
import { rightWords } from '../plugins/rights';
import { panelSafeMode } from '../plugins/loader';
import { Sheet, WinRow, WinSection } from '../ui/sheet';
import { ask } from '../ui/confirm';

/**
 * Вид пакета одним словом — на метке. Людям — не «плагин» и «приложение», а где оно работает (docs/ARCHITECTURE.md, «Как
 * называем и где работает»): плагин — в шлюзе, приложение со своей программой — на компьютере.
 */
export const KIND_TITLES: Record<PackageKind, string> = {
    get character() {
        return tr.catalog.kindCharacter;
    },
    get plugin() {
        return tr.catalog.kindPlugin;
    },
    get app() {
        return tr.catalog.kindApp;
    },
    get theme() {
        return tr.catalog.kindTheme;
    },
    get agent() {
        return tr.catalog.kindAgent;
    },
};

/** Работает только на колонке: в `runs` нет компьютера (AirPlay). Без `runs` — не знаем, считаем как раньше. */
export function onStationOnly(runs?: string[] | null): boolean {
    return !!runs && runs.length > 0 && !runs.includes('pc');
}

/**
 * Где работает приложение — словами для людей: «в шлюзе», «на компьютере», «на колонке» (приложение только для
 * колонки, AirPlay), «в сети» (MCP); персонаж и тема — как есть.
 */
export function placeOf(kind: PackageKind | null | undefined, runtime?: string | null, runs?: string[] | null): string {
    if (runtime === 'mcp') return tr.catalog.placeNetwork;
    if ((runtime === 'process' || kind === 'app') && onStationOnly(runs)) return tr.catalog.kindAppStation;
    if (runtime === 'process' || kind === 'app') return tr.catalog.kindApp;
    return KIND_TITLES[kind ?? 'plugin'] ?? tr.catalog.kindPlugin;
}

/** Может ли этот вход ставить и удалять пакеты такого вида: персонажа — с правом на характер, тему — на настройки, код — администратор. */
export function canManage(kind?: PackageKind | null): boolean {
    if (kind === 'character') return can('persona.edit');
    if (kind === 'theme' || kind === 'agent') return can('settings.edit');
    return can('plugins.manage') && sessionStore.get()?.role === 'admin';
}

/** Стоит ли пакет каталога здесь. */
export const isInstalled = (entry: CatalogEntry): boolean => !!entry.installed || entry.state === 'installed' || entry.state === 'update';

/** Можно ли его поставить: не пойдёт здесь и плагины без подписи — нет (серым, с причиной). */
export const isBlocked = (entry: CatalogEntry): boolean => entry.state === 'incompatible' || entry.state === 'needsSignature';

export interface CatalogState {
    view: CatalogView | null;
    /** Шлюз без каталога (постарше): экраны показывают только то, что стоит. */
    absent: boolean;
    error: string | null;
    refreshing: boolean;
    /** Перечитать; refresh — скачать ленту заново, не дожидаясь шести часов. */
    reload: (refresh?: boolean) => Promise<void>;
    setView: (view: CatalogView) => void;
}

/** Каталог пакетов с состоянием каждого; перечитывается, когда шлюз говорит о переменах (packages.changed). */
/** `station` — каталог центра дома через проход помощника (первые шаги на компьютере-помощнике, п. 46). */
export function useCatalog(station?: string): CatalogState {
    const panel = usePanel();
    const api = installApi(station);
    const [view, setView] = useState<CatalogView | null>(null);
    const [absent, setAbsent] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [refreshing, setRefreshing] = useState(false);

    const reload = async (refresh = false): Promise<void> => {
        if (refresh) setRefreshing(true);
        try {
            setView(await api.packagesCatalog(refresh));
            setError(null);
        } catch (e) {
            if (e instanceof ApiError && (e.status === 404 || e.status === 501)) setAbsent(true);
            else setError(problemOf(e));
        } finally {
            if (refresh) setRefreshing(false);
        }
    };
    useEffect(() => {
        if (!absent) void reload();
    }, [panel.version, station]);
    return { view, absent, error, refreshing, reload, setView };
}

/**
 * Модели голоса этого компьютера: стоит ли модель, что нужна пакету, — тогда «нужна модель» на плитке не пишется.
 * Шлюз на колонке моделей не ставит (501): тогда не знаем — и метку не ставим, окно пакета скажет само.
 */
export function useEngines(): Record<string, VoiceLabEngine['status']> | null {
    const [engines, setEngines] = useState<Record<string, VoiceLabEngine['status']> | null>(null);
    useEffect(() => {
        let alive = true;
        void api
            .voiceLab()
            .then((lab) => alive && setEngines(Object.fromEntries(lab.engines.map((engine) => [engine.id, engine.status]))))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, []);
    return engines;
}

/** Модель, нужная пакету, которой здесь нет (или null): для метки «нужна модель · 4 ГБ». */
export function missingEngine(entry: CatalogEntry, engines: Record<string, VoiceLabEngine['status']> | null) {
    if (!engines) return null;
    return (entry.needs ?? []).find((need) => need.kind === 'engine' && engines[need.id] && engines[need.id] !== 'ready') ?? null;
}

/** Метки пакета каталога: откуда он (дополнительная лента — её название), что с ним здесь. */
export function entryPills(entry: CatalogEntry, engines: Record<string, VoiceLabEngine['status']> | null): Array<{ text: string; cls: string }> {
    const pills: Array<{ text: string; cls: string }> = [];
    // Свой источник — заметно: его пакеты никто не проверял.
    if (entry.own) pills.push({ text: `${tr.catalog.pillOwnSource} · ${entry.source ?? ''}`.trim(), cls: 'pill-yandex' });
    else if (entry.source) pills.push({ text: entry.source, cls: 'pill-off' });
    if (entry.signed) pills.push({ text: tr.catalog.pillSigned, cls: 'pill-own' });
    if (entry.state === 'update') pills.push({ text: tr.catalog.pillNewVersion, cls: 'pill-agent' });
    else if (isInstalled(entry)) pills.push({ text: tr.catalog.pillInstalled, cls: 'pill-off' });
    if (entry.state === 'incompatible') pills.push({ text: tr.catalog.pillIncompatible, cls: 'pill-off' });
    if (entry.state === 'needsSignature') pills.push({ text: tr.catalog.pillNeedsSignature, cls: 'pill-off' });
    if (entry.verified === false) pills.push({ text: tr.catalog.pillUnverified, cls: 'pill-off' });
    const missing = !isInstalled(entry) && !isBlocked(entry) ? missingEngine(entry, engines) : null;
    if (missing) {
        const size = needsSize(missing);
        pills.push({ text: size ? tr.catalog.pillNeedsModelSize(size) : tr.catalog.pillNeedsModel, cls: 'pill-yandex' });
    }
    return pills;
}

export function rightsLines(entry: CatalogEntry): string[] {
    return (entry.permissions ?? []).map(rightWords);
}

/** Права на шине (`call:system/mic`, `see:files/*`) — их пульт спрашивает при установке, одним вопросом. */
export function busRights(entry: CatalogEntry): string[] {
    return (entry.permissions ?? []).filter((right) => /^(see|listen|call|edit):/.test(right));
}

/** Ставится только на колонку: ОЗУ в цене — колонки. */
function stationOnly(entry: CatalogEntry): boolean {
    return entry.place === 'station' || (!!entry.runs?.length && !entry.runs.includes('pc'));
}

/** Один вопрос при установке: зачем (у пары), сколько займёт памяти и ОЗУ, какие права — словами. */
function installText(entry: CatalogEntry, why?: string | null): string {
    const rights = rightsLines(entry);
    return [
        why ? `${why}.` : null,
        tr.catalog.pairCost(entry.sizeMb ?? null, entry.memoryMb ?? null, stationOnly(entry)),
        rights.length ? tr.catalog.installRights(rights.join(' · ')) : null,
    ]
        .filter(Boolean)
        .join(' ');
}

/**
 * «Нужна модель OmniVoice — на диске ~4 ГБ (можно и без неё)». Есть план голоса от Турбо (п. 40) — модель и размер
 * берутся из него, как в мастере установки: одно число на одну модель, а не каталожное рядом с Турбовым.
 */
function needsLines(entry: CatalogEntry, plan?: HomeVoicePlan | null): string[] {
    // Не помещается ни одна — всё равно число Турбо (советуемой), не каталожное: мастер покажет то же.
    const list = usableEngines(plan);
    const picked = list.find((engine) => engine.id === defaultEngine(plan)) ?? list.find((engine) => engine.recommended) ?? list[0];
    const engines = (entry.needs ?? []).filter((need) => need.kind === 'engine');
    if (picked && engines.length > 0) {
        // Число — тем же счётом, что в мастере (planGb), а уже стоящая модель — без числа: качать нечего.
        const line =
            picked.state === 'ready'
                ? `${tr.wizard.engineOnReady(picked.title)}.`
                : tr.catalog.needLine(tr.catalog.needVoiceModel(picked.title), tr.ui.sizeGb(planGb(picked.totalMb ?? picked.diskMb)), engines.some((need) => !!need.optional));
        return [line, ...needsLines({ ...entry, needs: (entry.needs ?? []).filter((need) => need.kind !== 'engine') })];
    }
    return (entry.needs ?? []).map((need) => {
        const size = needsSize(need);
        const what = need.kind === 'engine' ? tr.catalog.needVoiceModel(need.title ?? need.id) : tr.catalog.needFile(need.title ?? need.id);
        return tr.catalog.needLine(what, size || null, !!need.optional);
    });
}

export interface PackageActions {
    busy: Record<string, string>;
    notes: Record<string, CardNote>;
    say: (id: string, note: CardNote) => void;
    /**
     * Поставить; приложение или плагин из магазина — сразу и включить. Просит права на шине — сначала один вопрос: цена
     * и права словами, «Поставить» — и «да» на них, шлюз даёт их до первого запуска (`approved` — уже спросили, как у пары).
     */
    install: (entry: CatalogEntry, approved?: boolean) => Promise<boolean>;
    /** Включить поставленное, но выключенное приложение. */
    switchOn: (entry: CatalogEntry) => Promise<boolean>;
    update: (id: string, title: string) => Promise<boolean>;
    remove: (id: string, title: string, kind?: PackageKind | null) => Promise<boolean>;
}

/** Поставить, обновить, удалить: у каждого пакета своя занятость и свой итог; после — перечитать экран. */
/**
 * Приложение только что поставили или включили: его программа стартует и в первые секунды тянется к тому, что
 * попросила (сеть, плеер). Если «да» не дали с установкой (её делал не администратор дома или окно колонки открыто
 * через компьютер), шлюз спрашивает, а лист «да» узнаёт о вопросе по живому каналу. Канал мог не донести (окно колонки
 * `?via=`) — лист перечитывает вопросы сам, чтобы «да» было видно сразу (проверка начисто 18.11, п. 8).
 */
function soonAsked(): void {
    for (const after of [2000, 5000, 10_000]) window.setTimeout(busChanged, after);
}

/** `station` — ставит, включает, обновляет и удаляет у центра дома через проход помощника (п. 46). */
export function usePackageActions(onChanged: () => void, station?: string): PackageActions {
    const api = installApi(station);
    const [busy, setBusy] = useState<Record<string, string>>({});
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const say = (id: string, note: CardNote) => setNotes((all) => ({ ...all, [id]: note }));
    const run = async <T,>(id: string, what: string, action: () => Promise<T>, done: string | ((result: T) => string)): Promise<boolean> => {
        setBusy((all) => ({ ...all, [id]: what }));
        say(id, {});
        try {
            const result = await action();
            say(id, { notice: typeof done === 'string' ? done : done(result) });
            onChanged();
            return true;
        } catch (e) {
            say(id, { error: problemOf(e) });
            // Поставлено, но не включилось — экран всё равно перечитать: пакет уже стоит.
            onChanged();
            return false;
        } finally {
            setBusy((all) => {
                const next = { ...all };
                delete next[id];
                return next;
            });
        }
    };
    const install = async (entry: CatalogEntry, approved = false): Promise<boolean> => {
        // «Установить» человека — мастер (magicplace/wizard.tsx): что поставится и какие права одним вопросом, потом
        // шаги до «Скажите: …». Без мастера на экране (его нет только в тестах) — прежний вопрос.
        if (!approved) {
            const done = await openInstallWizard(entry, onChanged, false, station);
            if (done !== null) {
                if (done) soonAsked();
                onChanged();
                return done;
            }
        }
        const rights = busRights(entry).length > 0;
        if (rights && !approved && !(await ask({ title: tr.catalog.installTitle(entry.title), text: installText(entry), ok: tr.catalog.pairOk })))
            return false;
        const done = await run(entry.id, 'install', () => api.installPackage(entry.id, entry.version, true, rights), (view) =>
            view.enabled ? tr.catalog.installedOnNote : tr.catalog.installedNote,
        );
        if (done) {
            soonAsked();
            await offerPairs(entry, install, say, api);
        }
        return done;
    };
    return {
        busy,
        notes,
        say,
        install,
        switchOn: async (entry) => {
            const done = await run(entry.id, 'on', () => api.enablePackage(entry.id), tr.catalog.switchedOnNote);
            if (done) soonAsked();
            return done;
        },
        update: (id) => run(id, 'update', () => api.updatePackage(id), tr.catalog.updatedNote),
        remove: async (id, title, kind) => {
            const what = kind === 'character' ? tr.catalog.removeCharacterWhat : tr.catalog.removeWhat;
            if (!(await ask({ title: tr.catalog.removeTitle(title), text: tr.catalog.removeText(what), ok: tr.catalog.remove, danger: true })))
                return false;
            // Карточка агента: шлюз говорит, из каких рабочих папок агент ушёл.
            const done = await run(id, 'remove', () => api.removePackage(id), (result) => result?.said ?? tr.catalog.removedNote);
            if (done) await removePairs(id, (other) => run(other.id, 'remove', () => api.removePackage(other.id), tr.catalog.removedNote), api);
            return done;
        },
    };
}

/** Каталог сейчас: что стоит и что можно поставить — пары смотрят в него после установки и удаления. Каталог того, у
 *  кого ставили (`at` — у центра дома через проход, п. 46), а не этого пульта: иначе пару предлагали и удаляли по чужому
 *  каталогу (20.11). */
async function catalogNow(at: CatalogApi): Promise<CatalogEntry[]> {
    try {
        return (await at.packagesCatalog()).packages;
    } catch {
        return [];
    }
}

/** Чей каталог смотрят пары: `installApi(station)`. */
type CatalogApi = Pick<ReturnType<typeof installApi>, 'packagesCatalog'>;

/**
 * Поставили один из пары (`companions`: персонаж и его слово) — бесшовно предложить другой: зачем, сколько займёт
 * (память и ОЗУ), какие права. Отказались или нельзя (не влезло, не закреплено) — вернуться можно из карточки: строка
 * «В паре». Вторая установка предлагает свою пару сама — первую, уже стоящую, пропустит.
 */
async function offerPairs(
    entry: CatalogEntry,
    install: (entry: CatalogEntry, approved?: boolean) => Promise<boolean>,
    say: (id: string, note: CardNote) => void,
    at: CatalogApi,
) {
    if (!entry.companions?.length) return;
    const all = await catalogNow(at);
    for (const pair of entry.companions) {
        const other = all.find((item) => item.id === pair.id);
        if (!other || isInstalled(other) || !canManage(other.kind)) continue;
        if (other.state !== 'available') {
            if (other.reason) say(entry.id, { warning: tr.catalog.pairCannot(other.title, other.reason) });
            continue;
        }
        // Тот же один вопрос, что у установки: «Поставить» — и «да» на его права.
        if (await ask({ title: tr.catalog.pairTitle(other.title), text: installText(other, pair.why), ok: tr.catalog.pairOk, cancel: tr.catalog.pairLater }))
            await install(other, true);
    }
}

/** Удалили один из пары — спросить про другой, если он стоит: сам по себе он может быть и не нужен. */
async function removePairs(id: string, remove: (other: CatalogEntry) => Promise<boolean>, at: CatalogApi) {
    const all = await catalogNow(at);
    const removed = all.find((item) => item.id === id);
    for (const pair of removed?.companions ?? []) {
        const other = all.find((item) => item.id === pair.id);
        if (!other || !isInstalled(other) || !canManage(other.kind)) continue;
        const yes = await ask({
            title: tr.catalog.pairRemoveTitle(other.title),
            text: tr.catalog.pairRemoveText,
            ok: tr.catalog.remove,
            cancel: tr.catalog.pairLater,
            danger: true,
        });
        if (yes) await remove(other);
    }
}

/** Пакеты-пара в карточке: стоит ли, выключен, спит — или «поставить»: вернуться к отказу и к тому, что не влезло. */
export function Companions({ entry, all, actions }: { entry?: CatalogEntry; all?: CatalogEntry[]; actions: PackageActions }) {
    const pairs = (entry?.companions ?? []).flatMap((pair) => {
        const other = all?.find((item) => item.id === pair.id);
        return other ? [{ pair, other }] : [];
    });
    if (!pairs.length) return null;
    return (
        <div class="char-block">
            <span class="t-meta fw-700">{tr.catalog.pairHead}</span>
            {pairs.map(({ pair, other }) => {
                const installed = isInstalled(other);
                const status = !installed ? tr.catalog.pairMissing : other.asleep ? tr.catalog.pairAsleep : other.on === false ? tr.catalog.pairOff : tr.catalog.pairOn;
                const busy = actions.busy[other.id];
                return (
                    <div key={other.id} class="flex flex-wrap items-center gap-8">
                        <span class="t-body-sm">{other.title}</span>
                        <span class={`t-caption ${other.asleep ? 'c-pill-yandex-fg' : ''}`}>{status}</span>
                        {!installed && !isBlocked(other) && canManage(other.kind) && (
                            <Action tone="on" label={tr.catalog.pairInstall} busy={busy === 'install'} disabled={!!busy} onClick={() => void actions.install(other)} />
                        )}
                        {!installed && isBlocked(other) && other.reason && <span class="t-caption c-pill-yandex-fg">{other.reason}</span>}
                        {pair.why && <span class="t-caption w-full">{pair.why}</span>}
                        <NoteLine note={actions.notes[other.id]} />
                    </div>
                );
            })}
        </div>
    );
}

/** Картинка превью или значок пакета — круг 40. */
export function PackageThumb({ url }: { url?: string | null }) {
    const src = useMediaUrl(url);
    return <span class="pkg-thumb">{src ? <img src={src} alt="" /> : <IconPackage size={20} />}</span>;
}

/** Итог последнего нажатия под пакетом. */
export function NoteLine({ note }: { note?: CardNote }) {
    if (!note) return null;
    return (
        <>
            {note.notice && <span class="t-caption c-accent-text">{note.notice}</span>}
            {note.warning && (
                <span class="t-caption c-pill-yandex-fg" role="status">
                    {note.warning}
                </span>
            )}
            {note.error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {note.error}
                </span>
            )}
        </>
    );
}

/** Кнопки пакета каталога: поставить, обновить, удалить — по состоянию и праву на его вид. */
export function EntryButtons({ entry, actions }: { entry: CatalogEntry; actions: PackageActions }) {
    if (!canManage(entry.kind)) return null;
    const busy = actions.busy[entry.id];
    if (isBlocked(entry) && !isInstalled(entry)) return null;
    return (
        <>
            {entry.state === 'update' && (
                <Action tone="on" label={tr.catalog.update} busy={busy === 'update'} disabled={!!busy} onClick={() => void actions.update(entry.id, entry.title)} />
            )}
            {isInstalled(entry) ? (
                <Action label={tr.catalog.removeShort} busy={busy === 'remove'} disabled={!!busy} onClick={() => void actions.remove(entry.id, entry.title, entry.kind)} />
            ) : (
                <Action tone="on" label={tr.catalog.install} busy={busy === 'install'} disabled={!!busy} onClick={() => void actions.install(entry)} />
            )}
        </>
    );
}

const cap = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);

/**
 * Пакет каталога в своём окне (холст «Окна DuoWave», 17.11): шапка — значок, имя и метки (подпись, откуда, версия);
 * почему не ставится — плашкой сверху; описание, «Послушать», что понадобится, что берёт, лицензия — разделами;
 * «Установить», «Обновить» или «Открыть» — одна главная внизу справа; «Удалить» — в «Опасном».
 */
export function CatalogEntrySheet({
    entry,
    actions,
    engines,
    all,
    onClose,
    open,
    children,
}: {
    entry: CatalogEntry;
    actions: PackageActions;
    engines: Record<string, VoiceLabEngine['status']> | null;
    /** Весь каталог: строка «В паре» находит в нём пакеты-пару. */
    all?: CatalogEntry[];
    onClose: () => void;
    /** Куда открыть поставленное: окно приложения, карточку персонажа. */
    open?: { label: string; go: () => void } | null;
    /** Что ещё сказать в окне: факты агента, что скачается, «после установки…». */
    children?: preact.ComponentChildren;
}) {
    const samples = entry.preview?.samples ?? [];
    const pills = entryPills(entry, engines);
    const busy = actions.busy[entry.id];
    const manage = canManage(entry.kind);
    const installed = isInstalled(entry);
    const blocked = isBlocked(entry) && !installed;
    const rights = entry.permissions ?? [];
    const words = rightsLines(entry);
    // Персонаж: размер модели — из плана Турбо через центр, тот же, что покажет мастер. Плана нет — каталожный.
    const [voicePlan, setVoicePlan] = useState<HomeVoicePlan | null>(null);
    useEffect(() => {
        if (entry.kind !== 'character' || !(entry.needs ?? []).some((need) => need.kind === 'engine') || !can('station.view')) return;
        let alive = true;
        void api
            .homeVoicePlan({ package: entry.id })
            .then((fresh) => alive && setVoicePlan(fresh.plan ? fresh : null))
            .catch(() => undefined);
        return () => void (alive = false);
    }, [entry.id]);
    const needs = needsLines(entry, voicePlan);
    const spin = (what: string) => busy === what && <span class="spinner spinner-sm" aria-hidden="true" />;
    const footer = !manage ? (
        open ? (
            <button class="btn btn-main" onClick={open.go}>
                {open.label}
            </button>
        ) : undefined
    ) : blocked ? (
        <>
            <span class="win-dirty">{tr.catalog.pillIncompatible}</span>
            <span class="win-grow" />
            <button class="btn btn-main" disabled>
                {cap(tr.catalog.install)}
            </button>
        </>
    ) : !installed ? (
        <button class="btn btn-main" disabled={!!busy} aria-busy={busy === 'install' || undefined} onClick={() => void actions.install(entry)}>
            {spin('install')}
            {cap(tr.catalog.install)}
        </button>
    ) : entry.state === 'update' ? (
        <>
            {open && (
                <button class="btn btn-plain" onClick={open.go}>
                    {open.label}
                </button>
            )}
            <button class="btn btn-main" disabled={!!busy} aria-busy={busy === 'update' || undefined} onClick={() => void actions.update(entry.id, entry.title)}>
                {spin('update')}
                {cap(tr.catalog.update)}
            </button>
        </>
    ) : unfinishedInstall(entry.id) ? (
        // Мастер установки закрыли посреди: продолжить — мастер покажет, где остановились.
        <>
            {open && (
                <button class="btn btn-plain" onClick={open.go}>
                    {open.label}
                </button>
            )}
            <button class="btn btn-main" onClick={() => void openInstallWizard(entry, undefined, true)}>
                {tr.wizard.resume}
            </button>
        </>
    ) : open ? (
        <button class="btn btn-main" onClick={open.go}>
            {open.label}
        </button>
    ) : undefined;
    return (
        <Sheet
            title={entry.title}
            icon={<PackageThumb url={entry.preview?.image} />}
            meta={
                <>
                    {pills.map((pill) => (
                        <span key={pill.text} class={`pill ${pill.cls}`}>
                            {pill.text}
                        </span>
                    ))}
                    <span class="win-chip">{KIND_TITLES[entry.kind] ?? entry.kind}</span>
                    {entry.version && <span class="win-chip">{entry.version}</span>}
                </>
            }
            onClose={onClose}
            footer={footer}
        >
            {entry.reason && (
                <p class="win-callout m-0" role="status">
                    {entry.reason}
                </p>
            )}
            {entry.description && <p class="win-lead">{entry.description}</p>}
            {samples.length > 0 && (
                <WinSection title={tr.catalog.listenTitle}>
                    <div class="win-tags">
                        {samples.map((url, at) => (
                            <span key={url} class="win-tag gap-6">
                                <PlayButton id={`catalog:${entry.id}:${at}`} url={url} label={tr.catalog.phraseLabel(entry.title, at + 1)} className="char-play is-inline" />
                                {tr.catalog.phrase(at + 1)}
                            </span>
                        ))}
                    </div>
                </WinSection>
            )}
            {needs.length > 0 && (
                <WinSection title={tr.catalog.needsTitle} group>
                    {needs.map((line) => (
                        <WinRow key={line} title={line} />
                    ))}
                </WinSection>
            )}
            <Companions entry={entry} all={all} actions={actions} />
            {children}
            {rights.length > 0 && (
                <WinSection title={tr.plugins.takesTitle} group>
                    {rights.map((right, at) => (
                        <WinRow key={right} title={words[at] ?? right}>
                            <span class="win-code">{right}</span>
                        </WinRow>
                    ))}
                </WinSection>
            )}
            {(entry.license || entry.runtime === 'node' || entry.runtime === 'process') && (
                <WinSection title={tr.catalog.licenseTitle} group>
                    {entry.license && <WinRow title={entry.license} />}
                    {(entry.runtime === 'node' || entry.runtime === 'process') && !onStationOnly(entry.runs) && (
                        <WinRow title={tr.catalog.builtByComputer(entry.runtime === 'node', !!entry.signed)} />
                    )}
                </WinSection>
            )}
            {manage && installed && (
                <WinSection title={tr.ui.danger} group>
                    <WinRow title={cap(tr.catalog.removeShort)} sub={tr.plugins.removeAbout}>
                        <button class="btn btn-danger btn-sm" disabled={!!busy} aria-busy={busy === 'remove' || undefined} onClick={() => void actions.remove(entry.id, entry.title, entry.kind)}>
                            {cap(tr.catalog.removeShort)}
                        </button>
                    </WinRow>
                </WinSection>
            )}
            {!manage && <p class="win-note">{entry.kind === 'character' ? tr.catalog.onlyCharacterEditor : tr.catalog.onlyAdmin}</p>}
            <NoteLine note={actions.notes[entry.id]} />
        </Sheet>
    );
}

/** Что сказать о самом каталоге: не настроен, выключен, без сети, когда скачан. */
export function catalogStatus(catalog: CatalogState): { text: string; tone: 'meta' | 'warning' } | null {
    const view = catalog.view;
    if (catalog.absent) return { text: tr.catalog.statusAbsent, tone: 'meta' };
    if (catalog.error && !view) return { text: tr.catalog.statusError(catalog.error), tone: 'warning' };
    if (!view) return null;
    if (!view.configured) return { text: tr.catalog.statusNotConfigured, tone: 'meta' };
    if (!view.enabled) return { text: tr.catalog.statusDisabled, tone: 'meta' };
    if (view.offline) return { text: tr.catalog.statusOffline, tone: 'warning' };
    if (view.problem) return { text: view.problem, tone: 'warning' };
    return null;
}

/** Показывать ли пакеты каталога, которых здесь нет: без сети и с выключенным каталогом — нет (поставить их нельзя). */
export function catalogOpen(view: CatalogView | null): boolean {
    return !!view && view.configured && view.enabled && !view.offline;
}

/** Строки о дополнительных лентах, которые сейчас не прочитались; пока каталог закрыт — о нём уже сказано, их нет. О своих источниках говорит их блок. */
export function feedLines(view: CatalogView | null): string[] {
    if (!catalogOpen(view)) return [];
    return (view?.feeds ?? [])
        .filter((feed) => !feed.own && (feed.offline || feed.problem))
        .map((feed) =>
            feed.offline || !feed.problem
                ? tr.catalog.feedOffline(feed.title, feed.count > 0)
                : feed.problem,
        );
}

/** Сколько своих источников можно добавить — столько же держит шлюз. */
const MAX_SOURCES = 4;

/** Что сказать о своём источнике одной строкой. */
function sourceLine(feed: CatalogSource): { text: string; warning: boolean } {
    if (feed.closed) return { text: tr.catalog.sourceClosed, warning: true };
    if (feed.offline) return { text: tr.catalog.sourceOffline(feed.count > 0), warning: true };
    if (feed.problem) return { text: feed.problem, warning: true };
    const count = tr.catalog.sourceCount(feed.count);
    return { text: feed.fetchedAt ? tr.catalog.sourceFetched(count, timeAgo(feed.fetchedAt)) : count, warning: false };
}

/**
 * «Свои источники»: ленты пакетов, добавленные здесь по ссылке (её дают лично — показать персонажа, проверить пакет).
 * Их пакеты никто не проверял: сначала предупреждение, ставятся только персонажи и темы. Только администратор.
 */
export function OwnSources({ view, onView }: { view: CatalogView | null; onView: (view: CatalogView) => void }) {
    const [link, setLink] = useState('');
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const own = (view?.feeds ?? []).filter((feed) => feed.own);
    const full = own.length >= MAX_SOURCES;

    const add = async () => {
        const url = link.trim();
        let host: string;
        try {
            host = new URL(url).host;
        } catch {
            setError(tr.catalog.notALink);
            return;
        }
        const yes = await ask({
            title: tr.catalog.addSourceTitle(host),
            text: tr.catalog.addSourceText,
            ok: tr.catalog.add,
            danger: true,
        });
        if (!yes) return;
        setBusy('add');
        setError(null);
        try {
            onView(await api.addPackageSource(url));
            setLink('');
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    const remove = async (feed: CatalogSource) => {
        const yes = await ask({
            title: tr.catalog.removeSourceTitle(feed.title),
            text: tr.catalog.removeSourceText,
            ok: tr.catalog.removeSource,
            danger: true,
        });
        if (!yes) return;
        setBusy(feed.url);
        setError(null);
        try {
            onView(await api.removePackageSource(feed.url));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    return (
        <div class="pkg-own" aria-labelledby="own-sources-h">
            <span id="own-sources-h" class="fw-700">
                {tr.catalog.ownSources}
            </span>
            <span class="t-caption">{tr.catalog.ownSourcesNote}</span>
            {own.map((feed) => {
                const line = sourceLine(feed);
                return (
                    <div key={feed.url} class="pkg-row">
                        <div class="pkg-main">
                            <span class="pkg-text">
                                <span class="pkg-title">
                                    {feed.title}
                                    <span class="pill pill-yandex">{tr.catalog.pillUnverified}</span>
                                </span>
                                <span class="t-caption break-all">{feed.url}</span>
                                <span class={`t-caption${line.warning ? ' c-pill-yandex-fg' : ''}`} role={line.warning ? 'status' : undefined}>
                                    {line.text}
                                </span>
                            </span>
                            <div class="pkg-actions">
                                <Action label={tr.catalog.removeShortLower} busy={busy === feed.url} disabled={busy !== null} onClick={() => void remove(feed)} />
                            </div>
                        </div>
                    </div>
                );
            })}
            {full ? (
                <span class="t-caption">{tr.catalog.sourcesFull(MAX_SOURCES)}</span>
            ) : (
                <form
                    class="flex gap-8 items-center"
                    // Enter в поле сам нажимает «добавить» (кнопка формы): здесь — только не перезагрузить страницу,
                    // иначе предупреждение всплывало дважды.
                    onSubmit={(e) => e.preventDefault()}
                >
                    <input
                        class="field flex-1 min-w-0"
                        type="url"
                        inputMode="url"
                        autocomplete="off"
                        aria-label={tr.catalog.sourceLinkLabel}
                        placeholder="https://…/catalog.json"
                        value={link}
                        onInput={(e) => {
                            setLink((e.target as HTMLInputElement).value);
                            setError(null);
                        }}
                    />
                    <Action label={tr.catalog.addLower} busy={busy === 'add'} disabled={!link.trim() || busy !== null} onClick={() => void add()} />
                </form>
            )}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}

/** Состояние плагина одним словом. */
const pluginStates = (): Record<string, { text: string; cls: string }> => ({
    loaded: { text: tr.catalog.stateLoaded, cls: 'pill-own' },
    ready: { text: tr.catalog.stateReady, cls: 'pill-own' },
    off: { text: tr.catalog.stateOff, cls: 'pill-off' },
    error: { text: tr.catalog.stateError, cls: 'pill-error' },
    safeMode: { text: tr.catalog.stateSafeMode, cls: 'pill-off' },
});

/** Фильтр установленного по происхождению: свои, встроенные (и системные), из каталога. */
type OriginFilter = 'all' | 'user' | 'bundled' | 'catalog';

const originFilters = (): Array<{ id: OriginFilter; label: string }> => [
    { id: 'all', label: tr.catalog.filterAll },
    { id: 'user', label: tr.catalog.filterUser },
    { id: 'bundled', label: tr.catalog.filterBundled },
    { id: 'catalog', label: tr.catalog.filterCatalog },
];

const originOf = (entry: PackageView): PackageOrigin => entry.origin ?? (entry.source === 'catalog' ? 'catalog' : 'user');
const filterOf = (entry: PackageView): OriginFilter => {
    const origin = originOf(entry);
    return origin === 'system' ? 'bundled' : origin;
};
/** Свои — первыми, потом из каталога, потом встроенные; внутри — как отдал шлюз. */
const ORIGIN_ORDER: Record<OriginFilter, number> = { user: 0, catalog: 1, bundled: 2, all: 3 };

/**
 * «Приложения» → «Установлено»: что стоит (плагины включаются здесь же; свои — первыми, фильтр «свои · встроенные · из
 * каталога»). Окно пакета — по ссылке `#/skills/installed/<id>`; ссылка на ещё не поставленный открывает его окно
 * каталога. Сам каталог — MagicPlace.
 */
/** Одна строка состояния плитки «Моих»: ошибка, новая версия, ждёт ключ — или как работает. */
function tileStatus(entry: PackageView, fromCatalog: CatalogEntry | undefined, station: boolean): { text: string; tone: 'ok' | 'off' | 'bad' | 'new' | 'wait' } {
    const kindOf = entry.kind ?? 'plugin';
    const runs = kindOf === 'plugin' || kindOf === 'app';
    // Только для компьютера, а стоит у центра на колонке (встроенные «Папки»): не ошибка — здесь ему просто не место
    // (проверка начисто 18.11, п. 10). Шлюз пишет «не работает здесь…» в `problem`; плитка говорит, где оно работает.
    if (entry.problem && station && elsewhere(entry.runs)) return { text: tr.catalog.kindApp, tone: 'off' };
    if (entry.problem) return { text: tr.skills.stateProblem, tone: 'bad' };
    if (fromCatalog?.state === 'update') return { text: tr.catalog.pillNewVersion, tone: 'new' };
    if ((entry.waits?.length ?? 0) > 0) return { text: tr.core.waitsKey, tone: 'wait' };
    if (!runs) return { text: KIND_TITLES[kindOf] ?? kindOf, tone: 'ok' };
    if (!entry.enabled) return { text: tr.catalog.stateOff, tone: 'off' };
    if (entry.state === 'error') return { text: tr.catalog.stateError, tone: 'bad' };
    return { text: pluginStates()[entry.state]?.text ?? tr.catalog.stateReady, tone: entry.state === 'off' || entry.state === 'safeMode' ? 'off' : 'ok' };
}

/** Приложение только для компьютера: ни колонки, ни шлюза на колонке среди мест, где оно работает. */
const elsewhere = (runs?: string[] | null): boolean => !!runs && runs.length > 0 && runs.every((place) => place === 'pc');

export function PackagesTab() {
    const panel = usePanel();
    const catalog = useCatalog();
    const engines = useEngines();
    const linked = useLinkedItem('skills', 'installed');
    const [installed, setInstalled] = useState<PackagesView | null>(null);
    const [installedError, setInstalledError] = useState<string | null>(null);
    const [origin, setOrigin] = useState<OriginFilter>('all');
    /** Открытое окно установленного пакета — по ссылке вкладки. */
    const opened = linked.item;
    const isAdmin = can('plugins.manage') && sessionStore.get()?.role === 'admin';

    const readInstalled = () =>
        api
            .packages()
            .then((view) => {
                setInstalled(view);
                setInstalledError(null);
            })
            .catch((e) => setInstalledError(problemOf(e)));
    useEffect(() => {
        if (can('connectors.view')) void readInstalled();
    }, [panel.version]);

    const changed = () => {
        void catalog.reload();
        if (can('connectors.view')) void readInstalled();
    };
    const actions = usePackageActions(changed);


    const view = catalog.view;
    const entries = view?.packages ?? [];
    const byId = new Map(entries.map((entry) => [entry.id, entry]));
    // Ссылка вела на ещё не поставленный пакет: его окно каталога.
    const linkedEntry = linked.item && !installed?.packages.some((entry) => entry.id === linked.item) ? byId.get(linked.item) : undefined;
    // Персонажи и темы — не приложения: их выбирают в «Персонаже» и в «Оформлении» (docs/plans/menu-scheme.md).
    const all = (installed?.packages ?? []).filter((entry) => entry.kind !== 'character' && entry.kind !== 'theme');
    const counts = Object.fromEntries(originFilters().map((filter) => [filter.id, filter.id === 'all' ? all.length : all.filter((entry) => filterOf(entry) === filter.id).length]));
    const listed = all
        .map((entry, index) => ({ entry, index }))
        .filter(({ entry }) => origin === 'all' || filterOf(entry) === origin)
        .sort((a, b) => ORIGIN_ORDER[filterOf(a.entry)] - ORIGIN_ORDER[filterOf(b.entry)] || a.index - b.index)
        .map(({ entry }) => entry);

    return (
        <>
            {can('connectors.view') && (
                <section class="card glass card-list card-apps" aria-labelledby="packages-h">
                    <h2 id="packages-h" class="sr-only">
                        {tr.catalog.installedTitle}
                    </h2>
                    {installed?.safeMode && <p class="t-caption m-0 mb-6">{tr.catalog.safeModeApps}</p>}
                    {installedError && (
                        <p class="t-caption m-0 c-dot-offline" role="alert">
                            {installedError}
                        </p>
                    )}
                    {panelSafeMode() && (
                        <p class="t-caption m-0 mb-6 c-pill-yandex-fg" role="status">
                            {tr.catalog.panelSafeMode}
                        </p>
                    )}
                    {installed && installed.packages.some((entry) => entry.permissions.includes('schedule')) && <SchedulesSwitchLine />}
                    <LaterLine />
                    {installed && all.length === 0 && (
                        <p class="t-caption m-0 mb-6">
                            {tr.catalog.nothingInstalledBefore}{' '}
                            <button class="underline cursor-pointer" onClick={() => panel.go('magicplace')}>
                                {tr.catalog.inCatalog}
                            </button>
                            .
                        </p>
                    )}
                    {all.length > 0 && originFilters().filter((filter) => filter.id === 'all' || counts[filter.id] > 0).length > 2 && (
                        <div class="pkg-chips" role="group" aria-label={tr.catalog.originGroup}>
                            {originFilters().filter((filter) => filter.id === 'all' || counts[filter.id] > 0).map((filter) => (
                                <button
                                    key={filter.id}
                                    class={`pill ${origin === filter.id ? 'pill-pick' : 'pill-off'} cursor-pointer min-h-32`}
                                    aria-pressed={origin === filter.id}
                                    onClick={() => setOrigin(filter.id)}
                                >
                                    {filter.label} <span class="opacity-70">{counts[filter.id]}</span>
                                </button>
                            ))}
                        </div>
                    )}
                    <div class="app-tiles">
                        {/* Пока список читается — плитки-скелеты той же высоты: пришёл, ничего не сдвинулось. */}
                        {!installed &&
                            !installedError &&
                            [0, 1, 2, 3].map((at) => (
                                <span key={`sk${at}`} class="app-tile" aria-hidden="true">
                                    <span class="sk sk-ico" style={{ width: '40px', height: '40px' }} />
                                    <span class="app-tile-text sk-lines" style={{ display: 'flex', flexDirection: 'column', flex: 1 }}>
                                        <span class="sk sk-line" style={{ width: `${[56, 44, 62, 50][at]}%` }} />
                                        <span class="sk sk-line sk-thin" style={{ width: '40%' }} />
                                    </span>
                                </span>
                            ))}
                        {listed.map((entry) => {
                            const kindOf = entry.kind ?? 'plugin';
                            const fromCatalog = byId.get(entry.id);
                            const status = tileStatus(entry, fromCatalog, installed?.placement === 'station' || installed?.placement === 'stationServer');
                            return (
                                <button key={entry.id} class="app-tile" onClick={() => linked.open(entry.id)}>
                                    <AppIcon id={entry.id} title={entry.title} image={fromCatalog?.preview?.image} />
                                    <span class="app-tile-text">
                                        <span class="app-tile-title">{entry.title}</span>
                                        <span class={`app-tile-state is-${status.tone}`}>{status.text}</span>
                                    </span>
                                    <span class="sr-only">{KIND_TITLES[kindOf] ?? kindOf}</span>
                                </button>
                            );
                        })}
                    </div>
                    {listed.map((entry) => (
                        <NoteLine key={entry.id} note={actions.notes[entry.id]} />
                    ))}
                    {!isAdmin && installed?.packages.some((entry) => (entry.kind ?? 'plugin') === 'plugin') && (
                        <p class="t-caption m-0 mt-8">{tr.catalog.adminTurnsOn}</p>
                    )}
                </section>
            )}
            {/* Один список дома (docs/plans/home-and-devices.md, этап 5а): что стоит у Турбо дома. */}
            {can('station.view') && <OtherDevicesApps />}

            {opened && installed?.packages.find((entry) => entry.id === opened) && (
                <InstalledPackageSheet
                    entry={installed.packages.find((entry) => entry.id === opened)!}
                    onClose={linked.close}
                    onChanged={changed}
                    update={
                        byId.get(opened)?.state === 'update' && canManage(byId.get(opened)?.kind)
                            ? { busy: actions.busy[opened] === 'update', run: () => void actions.update(opened, byId.get(opened)?.title ?? opened) }
                            : undefined
                    }
                    onRemove={(entry) =>
                        void actions.remove(entry.id, entry.title, entry.kind).then((removed) => {
                            if (removed) linked.close();
                        })
                    }
                />
            )}

            {linkedEntry && (
                <CatalogEntrySheet
                    entry={linkedEntry}
                    actions={actions}
                    engines={engines}
                    all={view?.packages}
                    onClose={linked.close}
                    open={linkedEntry.kind === 'character' && isInstalled(linkedEntry) ? { label: tr.catalog.openCharacter, go: () => panel.go('character', 'characters', linkedEntry.id) } : null}
                />
            )}
        </>
    );
}
