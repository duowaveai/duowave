// Пакеты и общий каталог — один магазин для всех видов: персонажи, плагины, приложения, темы. «Плагины» → «Установлено»
// показывает, что стоит (свои — первыми, с фильтром по происхождению), «Плагины» → «Каталог» — ленту с фильтром по виду; «Мозг» → «Персонажи» — тот же каталог, отфильтрованный по
// виду character, на тех же хуках, метках и окне пакета отсюда. Каталог сам ничего не ставит и не обновляет: только
// кнопка человека с правом на этот вид (шлюз всё равно проверяет каждое действие сам).

import { useEffect, useState } from 'preact/hooks';

import { ApiError, CatalogEntry, CatalogView, PackageKind, PackageOrigin, PackageView, PackagesView, VoiceLabEngine, api, can, problemOf, sessionStore } from '../api';
import { IconPackage } from '../icons';
import { useLinkedItem, usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { Action, CardNote } from '../ui/cards';
import { Dropdown } from '../ui/dropdown';
import { PlayButton, useMediaUrl } from '../ui/media';
import { needsSize } from '../ui/needs';
import { pluginMemoryLine } from './memory';
import { InstalledPackageSheet, ORIGIN_TITLES } from './plugins';
import { panelSafeMode } from '../plugins/loader';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { ask } from '../ui/confirm';

/** Вид пакета одним словом — на метке. */
export const KIND_TITLES: Record<PackageKind, string> = {
    character: 'персонаж',
    plugin: 'плагин',
    app: 'приложение',
    theme: 'тема',
};

const KIND_FILTERS: Array<{ id: PackageKind; label: string }> = [
    { id: 'character', label: 'Персонажи' },
    { id: 'plugin', label: 'Навыки' },
    { id: 'app', label: 'Приложения' },
    { id: 'theme', label: 'Темы' },
];

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
    if (entry.source) pills.push({ text: entry.source, cls: 'pill-off' });
    if (entry.state === 'update') pills.push({ text: 'новая версия', cls: 'pill-agent' });
    else if (isInstalled(entry)) pills.push({ text: 'установлен', cls: 'pill-off' });
    if (entry.state === 'incompatible') pills.push({ text: 'не пойдёт здесь', cls: 'pill-off' });
    if (entry.state === 'needsSignature') pills.push({ text: 'нужна подпись', cls: 'pill-off' });
    if (entry.verified === false) pills.push({ text: 'не проверен', cls: 'pill-off' });
    const missing = !isInstalled(entry) && !isBlocked(entry) ? missingEngine(entry, engines) : null;
    if (missing) {
        const size = needsSize(missing);
        pills.push({ text: size ? `нужна модель · ${size}` : 'нужна модель', cls: 'pill-yandex' });
    }
    return pills;
}

/** «Нужна модель OmniVoice — на диске ~4 ГБ (можно и без неё)». */
function needsLines(entry: CatalogEntry): string[] {
    return (entry.needs ?? []).map((need) => {
        const size = needsSize(need);
        const what = need.kind === 'engine' ? `модель голоса ${need.title ?? need.id}` : `файл ${need.title ?? need.id}`;
        return `Понадобится ${what}${size ? ` — ~${size} на диске` : ''}${need.optional ? '; без неё говорит запасной голос' : ''}.`;
    });
}

export interface PackageActions {
    busy: Record<string, string>;
    notes: Record<string, CardNote>;
    say: (id: string, note: CardNote) => void;
    install: (entry: CatalogEntry) => Promise<boolean>;
    update: (id: string, title: string) => Promise<boolean>;
    remove: (id: string, title: string, kind?: PackageKind | null) => Promise<boolean>;
}

/** Поставить, обновить, удалить: у каждого пакета своя занятость и свой итог; после — перечитать экран. */
export function usePackageActions(onChanged: () => void): PackageActions {
    const [busy, setBusy] = useState<Record<string, string>>({});
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const say = (id: string, note: CardNote) => setNotes((all) => ({ ...all, [id]: note }));
    const run = async (id: string, what: string, action: () => Promise<unknown>, done: string): Promise<boolean> => {
        setBusy((all) => ({ ...all, [id]: what }));
        say(id, {});
        try {
            await action();
            say(id, { notice: done });
            onChanged();
            return true;
        } catch (e) {
            say(id, { error: problemOf(e) });
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
        install: (entry) => run(entry.id, 'install', () => api.installPackage(entry.id, entry.version), 'Поставлен.'),
        update: (id) => run(id, 'update', () => api.updatePackage(id), 'Обновлён.'),
        remove: async (id, title, kind) => {
            const what = kind === 'character' ? 'Персонаж уйдёт вместе с его голосами.' : 'Его файлы и настройки уйдут.';
            if (!(await ask({ title: `Удалить «${title}»?`, text: `${what} Вернуть можно из каталога.`, ok: 'Удалить', danger: true })))
                return false;
            return run(id, 'remove', () => api.removePackage(id), 'Удалён.');
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
                <Action tone="on" label="обновить" busy={busy === 'update'} disabled={!!busy} onClick={() => void actions.update(entry.id, entry.title)} />
            )}
            {isInstalled(entry) ? (
                <Action label="удалить" busy={busy === 'remove'} disabled={!!busy} onClick={() => void actions.remove(entry.id, entry.title, entry.kind)} />
            ) : (
                <Action tone="on" label="установить" busy={busy === 'install'} disabled={!!busy} onClick={() => void actions.install(entry)} />
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
        <Sheet title={entry.title} about={`${KIND_TITLES[entry.kind] ?? entry.kind} · версия ${entry.version}`} icon={<PackageThumb url={entry.preview?.image} />} onClose={onClose}>
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
                            <PlayButton id={`catalog:${entry.id}:${at}`} url={url} label={`${entry.title}, фраза ${at + 1}`} className="char-play is-inline" />
                            фраза {at + 1}
                        </span>
                    ))}
                </div>
            )}
            {needsLines(entry).map((line) => (
                <p key={line} class="t-caption m-0">
                    {line}
                </p>
            ))}
            {entry.license && <p class="t-caption m-0">Лицензия: {entry.license}</p>}
            {!canManage(entry.kind) && (
                <p class="t-caption m-0">
                    {entry.kind === 'character' ? 'Ставить персонажей может тот, кому можно менять характер.' : 'Ставить такие пакеты может администратор.'}
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
    if (catalog.absent) return { text: 'Этот шлюз пока без каталога — показаны установленные.', tone: 'meta' };
    if (catalog.error && !view) return { text: `Каталог не прочитать: ${catalog.error}`, tone: 'warning' };
    if (!view) return null;
    if (!view.configured) return { text: 'Каталог не настроен в этой сборке — показаны установленные.', tone: 'meta' };
    if (!view.enabled) return { text: 'Каталог не загружается — показаны установленные.', tone: 'meta' };
    if (view.offline) return { text: 'Каталог недоступен, показаны установленные.', tone: 'warning' };
    if (view.problem) return { text: view.problem, tone: 'warning' };
    return null;
}

/** Показывать ли пакеты каталога, которых здесь нет: без сети и с выключенным каталогом — нет (поставить их нельзя). */
export function catalogOpen(view: CatalogView | null): boolean {
    return !!view && view.configured && view.enabled && !view.offline;
}

/** Строки о дополнительных лентах, которые сейчас не прочитались; пока каталог закрыт — о нём уже сказано, их нет. */
export function feedLines(view: CatalogView | null): string[] {
    if (!catalogOpen(view)) return [];
    return (view?.feeds ?? [])
        .filter((feed) => feed.offline || feed.problem)
        .map((feed) =>
            feed.offline || !feed.problem
                ? `Лента «${feed.title}» недоступна${feed.count > 0 ? ' — её пакеты показаны по последней копии' : ''}.`
                : feed.problem,
        );
}

/** Состояние плагина одним словом. */
const PLUGIN_STATE: Record<string, { text: string; cls: string }> = {
    loaded: { text: 'работает', cls: 'pill-own' },
    ready: { text: 'включён', cls: 'pill-own' },
    off: { text: 'выкл', cls: 'pill-off' },
    error: { text: 'ошибка', cls: 'pill-error' },
    safeMode: { text: 'безопасный режим', cls: 'pill-off' },
};

/** Фильтр установленного по происхождению: свои, встроенные (и системные), из каталога. */
type OriginFilter = 'all' | 'user' | 'bundled' | 'catalog';

const ORIGIN_FILTERS: Array<{ id: OriginFilter; label: string }> = [
    { id: 'all', label: 'Все' },
    { id: 'user', label: 'Свои' },
    { id: 'bundled', label: 'Встроенные' },
    { id: 'catalog', label: 'Из каталога' },
];

const originOf = (entry: PackageView): PackageOrigin => entry.origin ?? (entry.source === 'catalog' ? 'catalog' : 'user');
const filterOf = (entry: PackageView): OriginFilter => {
    const origin = originOf(entry);
    return origin === 'system' ? 'bundled' : origin;
};
/** Свои — первыми, потом из каталога, потом встроенные; внутри — как отдал шлюз. */
const ORIGIN_ORDER: Record<OriginFilter, number> = { user: 0, catalog: 1, bundled: 2, all: 3 };

/**
 * «Плагины» → «Установлено» и «Каталог»: что стоит (плагины включаются здесь же; свои — первыми, фильтр «свои ·
 * встроенные · из каталога») и лента каталога с фильтром по виду и выключателем. Окно пакета открывается по ссылке
 * `#/plugins/installed/<id>`, пакет каталога — `#/plugins/catalog/<id>`.
 */
export function PackagesTab({ part }: { part: 'installed' | 'catalog' }) {
    const panel = usePanel();
    const catalog = useCatalog();
    const engines = useEngines();
    const linked = useLinkedItem('skills', part);
    const [installed, setInstalled] = useState<PackagesView | null>(null);
    const [installedError, setInstalledError] = useState<string | null>(null);
    const [kind, setKind] = useState<string | null>(null);
    const [origin, setOrigin] = useState<OriginFilter>('all');
    const [switching, setSwitching] = useState<string | null>(null);
    const [catalogBusy, setCatalogBusy] = useState(false);
    const [catalogError, setCatalogError] = useState<string | null>(null);
    /** Открытое окно установленного пакета — по ссылке вкладки. */
    const opened = part === 'installed' ? linked.item : null;
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

    const toggle = async (entry: PackageView, on: boolean) => {
        setSwitching(entry.id);
        actions.say(entry.id, {});
        try {
            await (on ? api.enablePackage(entry.id) : api.disablePackage(entry.id));
            await readInstalled();
        } catch (e) {
            actions.say(entry.id, { error: problemOf(e) });
        } finally {
            setSwitching(null);
        }
    };

    const setCatalogOn = async (on: boolean) => {
        setCatalogBusy(true);
        setCatalogError(null);
        try {
            catalog.setView(await api.setPackagesCatalog(on));
        } catch (e) {
            setCatalogError(problemOf(e));
        } finally {
            setCatalogBusy(false);
        }
    };

    const view = catalog.view;
    const entries = view?.packages ?? [];
    const byId = new Map(entries.map((entry) => [entry.id, entry]));
    const open = catalogOpen(view);
    const shown = entries.filter((entry) => (!kind || entry.kind === kind) && (open || isInstalled(entry)));
    const status = catalogStatus(catalog);
    // Пакет каталога по ссылке; на вкладке «Установлено» — тот, которого здесь нет (ссылка вела на ещё не поставленный).
    const linkedEntry = linked.item && (part === 'catalog' || !installed?.packages.some((entry) => entry.id === linked.item)) ? byId.get(linked.item) : undefined;
    const kinds = KIND_FILTERS.filter((filter) => entries.some((entry) => entry.kind === filter.id));
    const all = installed?.packages ?? [];
    const counts = Object.fromEntries(ORIGIN_FILTERS.map((filter) => [filter.id, filter.id === 'all' ? all.length : all.filter((entry) => filterOf(entry) === filter.id).length]));
    const listed = all
        .map((entry, index) => ({ entry, index }))
        .filter(({ entry }) => origin === 'all' || filterOf(entry) === origin)
        .sort((a, b) => ORIGIN_ORDER[filterOf(a.entry)] - ORIGIN_ORDER[filterOf(b.entry)] || a.index - b.index)
        .map(({ entry }) => entry);

    return (
        <>
            {part === 'installed' && can('connectors.view') && (
                <section class="card glass card-list" aria-labelledby="packages-h">
                    <div class="card-head">
                        <h2 id="packages-h" class="t-title-xs m-0">
                            Установлено
                        </h2>
                        <span class="t-meta">{installed?.safeMode ? 'безопасный режим: плагины не работают' : installed ? `${installed.packages.length}` : ''}</span>
                    </div>
                    {installedError && (
                        <p class="t-caption m-0 c-dot-offline" role="alert">
                            {installedError}
                        </p>
                    )}
                    {panelSafeMode() && (
                        <p class="t-caption m-0 mb-6 c-pill-yandex-fg" role="status">
                            Безопасный режим пульта (?safe=1): темы пакетов и части плагинов в пульт не загружены.
                        </p>
                    )}
                    {installed && installed.packages.length === 0 && (
                        <p class="t-caption m-0 mb-6">
                            Пока ничего не поставлено — выберите{' '}
                            <button class="underline cursor-pointer" onClick={() => panel.go('skills', 'catalog')}>
                                в каталоге
                            </button>
                            .
                        </p>
                    )}
                    {all.length > 0 && ORIGIN_FILTERS.filter((filter) => filter.id === 'all' || counts[filter.id] > 0).length > 2 && (
                        <div class="pkg-chips" role="group" aria-label="Откуда пакет">
                            {ORIGIN_FILTERS.filter((filter) => filter.id === 'all' || counts[filter.id] > 0).map((filter) => (
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
                    {listed.map((entry) => {
                        const kindOf = entry.kind ?? 'plugin';
                        const fromCatalog = byId.get(entry.id);
                        // Плагин и приложение включаются и работают; персонаж и тема — данные.
                        const runs = kindOf === 'plugin' || kindOf === 'app';
                        const state = runs ? PLUGIN_STATE[entry.state] : undefined;
                        const from = originOf(entry);
                        const editable = isAdmin && (entry.editable ?? from === 'user') && (kindOf === 'plugin' || kindOf === 'theme');
                        const busy = actions.busy[entry.id];
                        return (
                            <div key={entry.id} class="pkg-row">
                                <div class="pkg-main">
                                    <PackageThumb url={fromCatalog?.preview?.image} />
                                    <button class="pkg-text text-left cursor-pointer" onClick={() => linked.open(entry.id)}>
                                        <span class="pkg-title">
                                            {entry.title}
                                            <span class="pill pill-off">{KIND_TITLES[kindOf] ?? kindOf}</span>
                                            {from !== 'catalog' && <span class={`pill ${from === 'user' ? 'pill-own' : 'pill-off'}`}>{ORIGIN_TITLES[from]}</span>}
                                            {state && <span class={`pill ${state.cls}`}>{state.text}</span>}
                                            {fromCatalog?.state === 'update' && <span class="pill pill-agent">новая версия</span>}
                                        </span>
                                        <span class="t-caption">
                                            {[entry.version && `версия ${entry.version}`, entry.license, entry.description].filter(Boolean).join(' · ')}
                                        </span>
                                    </button>
                                    <div class="pkg-actions">
                                        {kindOf === 'character' && (
                                            <Action label="открыть" onClick={() => panel.go('character', 'characters', entry.id)} />
                                        )}
                                        {editable && <Action label="в мастерской →" onClick={() => panel.go('skills', 'workshop', entry.id)} />}
                                        {fromCatalog?.state === 'update' && canManage(kindOf) && (
                                            <Action tone="on" label="обновить" busy={busy === 'update'} disabled={!!busy} onClick={() => void actions.update(entry.id, entry.title)} />
                                        )}
                                        {canManage(kindOf) && (entry.removable ?? true) && (
                                            <Action label="удалить" busy={busy === 'remove'} disabled={!!busy} onClick={() => void actions.remove(entry.id, entry.title, kindOf)} />
                                        )}
                                        {runs && isAdmin && (
                                            <Switch
                                                on={entry.enabled}
                                                label={`${entry.title}: ${entry.enabled ? 'выключить' : 'включить'}`}
                                                disabled={switching === entry.id || !!installed?.safeMode || (entry.enabled && entry.canDisable === false)}
                                                onChange={(on) => void toggle(entry, on)}
                                            />
                                        )}
                                    </div>
                                </div>
                                {pluginMemoryLine(entry) && <span class="t-caption">{pluginMemoryLine(entry)}</span>}
                                {entry.problem && (
                                    <span class="t-caption c-dot-offline" role="alert">
                                        {entry.problem}
                                    </span>
                                )}
                                {entry.lastError && entry.lastError.message !== entry.problem && (
                                    <span class="t-caption c-dot-offline">Последняя ошибка {timeAgo(entry.lastError.at)}: {entry.lastError.message}</span>
                                )}
                                <NoteLine note={actions.notes[entry.id]} />
                            </div>
                        );
                    })}
                    {!isAdmin && installed?.packages.some((entry) => (entry.kind ?? 'plugin') === 'plugin') && (
                        <p class="t-caption m-0 mt-8">Включать плагины может администратор.</p>
                    )}
                </section>
            )}

            {part === 'catalog' && (
            <section class="card glass card-list" aria-labelledby="catalog-h">
                <div class="card-head">
                    <h2 id="catalog-h" class="t-title-xs m-0">
                        Каталог
                    </h2>
                    {view?.configured && view.enabled && (
                        <button class="t-caption cursor-pointer underline min-h-44" disabled={catalog.refreshing} onClick={() => void catalog.reload(true)}>
                            {catalog.refreshing ? 'проверяю…' : view.fetchedAt ? `обновлён ${timeAgo(view.fetchedAt)}` : 'проверить'}
                        </button>
                    )}
                </div>
                <p class="t-caption m-0 mb-6">Проверенные нами персонажи, плагины и темы. Ставится только то, что вы выберете.</p>
                {status && (
                    <p class={`t-caption m-0 mb-6 ${status.tone === 'warning' ? 'c-pill-yandex-fg' : ''}`} role="status">
                        {status.text}
                    </p>
                )}
                {feedLines(view).map((line) => (
                    <p key={line} class="t-caption m-0 mb-6 c-pill-yandex-fg" role="status">
                        {line}
                    </p>
                ))}
                {kinds.length > 1 && (
                    <div class="pkg-filters">
                        <Dropdown
                            label="Вид"
                            anyLabel="Все виды"
                            value={kind}
                            options={kinds.map((filter) => ({ id: filter.id, label: filter.label, hint: String(entries.filter((entry) => entry.kind === filter.id).length) }))}
                            onPick={setKind}
                        />
                    </div>
                )}
                {view && shown.length === 0 && !status && <p class="t-caption m-0 mb-6">В каталоге пока пусто.</p>}
                {shown.map((entry) => {
                    const pills = entryPills(entry, engines);
                    return (
                        <div key={entry.id} class={`pkg-row${isBlocked(entry) ? ' is-dim' : ''}`}>
                            <div class="pkg-main">
                                <PackageThumb url={entry.preview?.image} />
                                <button class="pkg-text text-left cursor-pointer" onClick={() => linked.open(entry.id)}>
                                    <span class="pkg-title">
                                        {entry.title}
                                        <span class="pill pill-off">{KIND_TITLES[entry.kind] ?? entry.kind}</span>
                                        {pills.map((pill) => (
                                            <span key={pill.text} class={`pill ${pill.cls}`}>
                                                {pill.text}
                                            </span>
                                        ))}
                                    </span>
                                    <span class="t-caption">{[entry.description, `версия ${entry.version}`, entry.license].filter(Boolean).join(' · ')}</span>
                                    {entry.reason && <span class="t-caption">{entry.reason}</span>}
                                </button>
                                <div class="pkg-actions">
                                    <EntryButtons entry={entry} actions={actions} />
                                </div>
                            </div>
                            <NoteLine note={actions.notes[entry.id]} />
                        </div>
                    );
                })}
                {view?.configured && can('settings.edit') && (
                    <div class="pkg-switch">
                        <div class="flex-1 min-w-0 flex flex-col gap-2">
                            <span class="fw-700">Не загружать каталог</span>
                            <span class="t-caption">Запрос каталога сообщает нашему сайту адрес шлюза. Выключите — шлюз не будет ходить за ним.</span>
                        </div>
                        <Switch on={!view.enabled} label="Не загружать каталог" disabled={catalogBusy} onChange={(off) => void setCatalogOn(!off)} />
                    </div>
                )}
                {catalogError && (
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {catalogError}
                    </p>
                )}
            </section>
            )}

            {opened && installed?.packages.find((entry) => entry.id === opened) && (
                <InstalledPackageSheet
                    entry={installed.packages.find((entry) => entry.id === opened)!}
                    onClose={linked.close}
                    onChanged={changed}
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
                            <Action label="Открыть персонажа" onClick={() => panel.go('character', 'characters', linkedEntry.id)} />
                        </div>
                    )}
                </CatalogEntrySheet>
            )}
        </>
    );
}
