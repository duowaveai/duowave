// Пакеты и общий каталог: «Приложения» → «Установлено» показывает, что стоит (свои — первыми, с фильтром по
// происхождению); сам каталог — магазин MagicPlace (magicplace/*, docs/plans/magicplace.md), «Персонаж» → «Персонажи» —
// он же, отфильтрованный по виду character. Хуки, метки, окно пакета и «Свои источники» — отсюда, общие для них.
// Каталог сам ничего не ставит и не обновляет: только кнопка человека с правом на этот вид (шлюз всё равно проверяет
// каждое действие сам).

import { useEffect, useState } from 'preact/hooks';

import { ApiError, CatalogEntry, CatalogSource, CatalogView, PackageKind, PackageOrigin, PackageView, PackagesView, VoiceLabEngine, api, can, problemOf, sessionStore } from '../api';
import { IconPackage } from '../icons';
import { tr } from '../i18n';
import { useLinkedItem, usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { AppIcon } from '../ui/app-icon';
import { Action, CardNote } from '../ui/cards';
import { PlayButton, useMediaUrl } from '../ui/media';
import { needsSize } from '../ui/needs';
import { InstalledPackageSheet, LaterLine, SchedulesSwitchLine } from './plugins';
import { rightWords } from '../plugins/rights';
import { panelSafeMode } from '../plugins/loader';
import { Sheet } from '../ui/sheet';
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
    if (kind === 'theme') return can('settings.edit');
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
export function useCatalog(): CatalogState {
    const panel = usePanel();
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
    }, [panel.version]);
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

/** «Нужна модель OmniVoice — на диске ~4 ГБ (можно и без неё)». */
function needsLines(entry: CatalogEntry): string[] {
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
    /** Поставить; приложение или плагин из магазина — сразу и включить. */
    install: (entry: CatalogEntry) => Promise<boolean>;
    /** Включить поставленное, но выключенное приложение. */
    switchOn: (entry: CatalogEntry) => Promise<boolean>;
    update: (id: string, title: string) => Promise<boolean>;
    remove: (id: string, title: string, kind?: PackageKind | null) => Promise<boolean>;
}

/** Поставить, обновить, удалить: у каждого пакета своя занятость и свой итог; после — перечитать экран. */
export function usePackageActions(onChanged: () => void): PackageActions {
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
    return {
        busy,
        notes,
        say,
        install: (entry) =>
            run(entry.id, 'install', () => api.installPackage(entry.id, entry.version, true), (view) =>
                view.enabled ? tr.catalog.installedOnNote : tr.catalog.installedNote,
            ),
        switchOn: (entry) => run(entry.id, 'on', () => api.enablePackage(entry.id), tr.catalog.switchedOnNote),
        update: (id) => run(id, 'update', () => api.updatePackage(id), tr.catalog.updatedNote),
        remove: async (id, title, kind) => {
            const what = kind === 'character' ? tr.catalog.removeCharacterWhat : tr.catalog.removeWhat;
            if (!(await ask({ title: tr.catalog.removeTitle(title), text: tr.catalog.removeText(what), ok: tr.catalog.remove, danger: true })))
                return false;
            return run(id, 'remove', () => api.removePackage(id), tr.catalog.removedNote);
        },
    };
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

/** Пакет каталога в своём окне: превью и фразы, что понадобится, лицензия, и кнопки. */
export function CatalogEntrySheet({
    entry,
    actions,
    engines,
    onClose,
    children,
}: {
    entry: CatalogEntry;
    actions: PackageActions;
    engines: Record<string, VoiceLabEngine['status']> | null;
    onClose: () => void;
    /** Что добавить под кнопками: персонаж — «открыть карточку» после установки. */
    children?: preact.ComponentChildren;
}) {
    const samples = entry.preview?.samples ?? [];
    const pills = entryPills(entry, engines);
    return (
        <Sheet title={entry.title} about={tr.catalog.aboutEntry(KIND_TITLES[entry.kind] ?? entry.kind, entry.version)} icon={<PackageThumb url={entry.preview?.image} />} onClose={onClose}>
            {entry.description && <p class="t-body-sm m-0">{entry.description}</p>}
            {pills.length > 0 && (
                <div class="flex flex-wrap gap-6">
                    {pills.map((pill) => (
                        <span key={pill.text} class={`pill ${pill.cls}`}>
                            {pill.text}
                        </span>
                    ))}
                </div>
            )}
            {entry.reason && (
                <p class="t-caption m-0 c-pill-yandex-fg" role="status">
                    {entry.reason}
                </p>
            )}
            {samples.length > 0 && (
                <div class="flex flex-wrap items-center gap-8">
                    {samples.map((url, at) => (
                        <span key={url} class="inline-flex items-center gap-6 t-caption">
                            <PlayButton id={`catalog:${entry.id}:${at}`} url={url} label={tr.catalog.phraseLabel(entry.title, at + 1)} className="char-play is-inline" />
                            {tr.catalog.phrase(at + 1)}
                        </span>
                    ))}
                </div>
            )}
            {needsLines(entry).map((line) => (
                <p key={line} class="t-caption m-0">
                    {line}
                </p>
            ))}
            {entry.license && <p class="t-caption m-0">{tr.catalog.license(entry.license)}</p>}
            {(entry.runtime === 'node' || entry.runtime === 'process') && !onStationOnly(entry.runs) && (
                <p class="t-caption m-0">{tr.catalog.builtByComputer(entry.runtime === 'node', !!entry.signed)}</p>
            )}
            {rightsLines(entry).length > 0 && <p class="t-caption m-0">{tr.catalog.rightsLine(rightsLines(entry).join(' · '))}</p>}
            {!canManage(entry.kind) && (
                <p class="t-caption m-0">
                    {entry.kind === 'character' ? tr.catalog.onlyCharacterEditor : tr.catalog.onlyAdmin}
                </p>
            )}
            <div class="flex flex-wrap items-center gap-8">
                <EntryButtons entry={entry} actions={actions} />
            </div>
            <NoteLine note={actions.notes[entry.id]} />
            {children}
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
function tileStatus(entry: PackageView, fromCatalog: CatalogEntry | undefined): { text: string; tone: 'ok' | 'off' | 'bad' | 'new' | 'wait' } {
    const kindOf = entry.kind ?? 'plugin';
    const runs = kindOf === 'plugin' || kindOf === 'app';
    if (entry.problem) return { text: tr.skills.stateProblem, tone: 'bad' };
    if (fromCatalog?.state === 'update') return { text: tr.catalog.pillNewVersion, tone: 'new' };
    if ((entry.waits?.length ?? 0) > 0) return { text: tr.core.waitsKey, tone: 'wait' };
    if (!runs) return { text: KIND_TITLES[kindOf] ?? kindOf, tone: 'ok' };
    if (!entry.enabled) return { text: tr.catalog.stateOff, tone: 'off' };
    if (entry.state === 'error') return { text: tr.catalog.stateError, tone: 'bad' };
    return { text: pluginStates()[entry.state]?.text ?? tr.catalog.stateReady, tone: entry.state === 'off' || entry.state === 'safeMode' ? 'off' : 'ok' };
}

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
                                    class={`pill ${origin === filter.id ? 'pill-own' : 'pill-off'} cursor-pointer min-h-36`}
                                    aria-pressed={origin === filter.id}
                                    onClick={() => setOrigin(filter.id)}
                                >
                                    {filter.label} <span class="opacity-70">{counts[filter.id]}</span>
                                </button>
                            ))}
                        </div>
                    )}
                    <div class="app-tiles">
                        {listed.map((entry) => {
                            const kindOf = entry.kind ?? 'plugin';
                            const fromCatalog = byId.get(entry.id);
                            const status = tileStatus(entry, fromCatalog);
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
                <CatalogEntrySheet entry={linkedEntry} actions={actions} engines={engines} onClose={linked.close}>
                    {linkedEntry.kind === 'character' && isInstalled(linkedEntry) && (
                        <div>
                            <Action label={tr.catalog.openCharacter} onClick={() => panel.go('character', 'characters', linkedEntry.id)} />
                        </div>
                    )}
                </CatalogEntrySheet>
            )}
        </>
    );
}
