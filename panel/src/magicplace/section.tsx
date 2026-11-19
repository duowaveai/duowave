// MagicPlace — встроенный магазин DuoWave (docs/plans/magicplace.md), как магазин приложений на телефоне: всё из лент
// каталога — приложения, персонажи, темы, — поиск по загруженному списку на всех языках пакета, витрина
// («Обновления», «Рекомендуем», «Новое») и весь список с фильтрами: вид, лицо приложения, где работает, состояние.
// «Установить», «Обновить» и «Открыть» — прямо здесь; окно пакета — по ссылке `#/magicplace/store/<id>` (`?install` —
// так ведут сайт каталога и голос). Свои источники и выключатель каталога — в настройках (шестерёнка).
//
// MagicPlace — не пакет, а часть пульта и шлюза: его не выключить и не удалить, своих прав ставить у него нет — он
// ходит в те же ручки, что кнопки каталога, с правами вошедшего (шлюз проверяет каждое действие сам).

import { WinRow, WinSection } from '../ui/sheet';
import { useEffect, useState } from 'preact/hooks';

import type { AgentCard, CatalogEntry, CatalogFace, CatalogPlace, VoiceLabEngine } from '../api';
import { tr } from '../i18n';
import { IconClose, IconGear, IconSearch } from '../icons';
import { useLinkedItem, usePanel } from '../shell/context';
import type { PanelState } from '../shell/context';
import { registeredPlugins } from '../shell/registry';
import { Action } from '../ui/cards';
import { Dropdown } from '../ui/dropdown';
import {
    CatalogEntrySheet,
    NoteLine,
    PackageActions,
    PackageThumb,
    canManage,
    catalogOpen,
    catalogStatus,
    entryPills,
    feedLines,
    isBlocked,
    isInstalled,
    placeOf,
    useCatalog,
    useEngines,
    usePackageActions,
} from '../screens/catalog';
import {
    Filters,
    KindFilter,
    NO_FILTERS,
    StateFilter,
    faceFilters,
    faceTitle,
    filterWords,
    filtering,
    filtersFrom,
    goesHere,
    isNew,
    kindFilters,
    kindOf,
    kindTitle,
    passes,
    placeFilters,
    recommendedHere,
    shownAlone,
    wordOfEntry,
    placeOfEntry,
    placeTitle,
    stateFilters,
    stateOf,
    stateTitle,
} from './filters';
import { MagicPlaceMark } from './icon';
import { PlacePill } from '../screens/home-apps';
import { AppsTabs } from '../screens/apps-tabs';
import { matchesQuery } from './search';
import { StoreSettings } from './settings';
import { noteStoreChange } from './state';
import './magicplace.css';

/** Вкладка раздела — одна, магазин: пакет открыт — `#/magicplace/store/<id>`. */
export const STORE_TAB = 'store';

type Engines = Record<string, VoiceLabEngine['status']> | null;

/**
 * «Открыть» поставленное: окно приложения (его раздел в пульте), карточку персонажа, «Оформление» у темы; приложение без
 * своего окна — его лист в «Приложения» → «Установлено». Не поставлено — открывать нечего.
 */
export function openTarget(entry: CatalogEntry, go: PanelState['go']): (() => void) | null {
    if (!isInstalled(entry)) return null;
    if (entry.kind === 'character') return () => go('character', 'characters', entry.id);
    if (entry.kind === 'theme') return () => go('settings', 'look');
    // Агент выбирается в рабочих папках: там видно, найдена ли его программа и вошли ли вы.
    if (entry.kind === 'agent') return () => go('works', 'folders');
    const plugin = registeredPlugins().find((plugin) => plugin.id === entry.id || plugin.id === `package.${entry.id}`);
    const section = plugin?.sections?.find((section) => section.nav !== 'link') ?? plugin?.sections?.[0];
    if (section) return () => go(section.id);
    return () => go('skills', 'installed', entry.id);
}

/**
 * Карточка агента до установки: установка ставит только карточку — программу агента ставите вы сами (командой), входите
 * в неё сами; ссылка на его условия; агент без вопросов перед командами помечен.
 */
export function AgentFacts({ card, title }: { card: AgentCard; title: string }) {
    return (
        <div class="mp-agent flex flex-col gap-4">
            <p class="t-caption m-0">{tr.magicplace.agentCardOnly}</p>
            {card.program.install && (
                <>
                    <p class="t-caption m-0">{tr.magicplace.agentInstall(card.program.name)}</p>
                    <code class="mp-agent-code">{card.program.install}</code>
                </>
            )}
            <p class="t-caption m-0">{card.signin.hint ?? tr.magicplace.agentSignin(title)}</p>
            {card.asks === false && <p class="t-caption m-0 mp-agent-warn">{tr.magicplace.agentNoQuestions}</p>}
            {card.model.via === 'duowave' && <p class="t-caption m-0">{tr.magicplace.agentViaDuowave}</p>}
            {card.terms && (
                <a class="t-caption c-accent-text" href={card.terms} target="_blank" rel="noopener noreferrer">
                    {tr.magicplace.agentTerms(title)}
                </a>
            )}
        </div>
    );
}

/** Что пакет скачает сам, словами: «Один раз скачает Node — около 50 МБ». */
export function downloadLines(entry: CatalogEntry): string[] {
    return (entry.downloads ?? []).map((download) =>
        download.when === 'install'
            ? tr.magicplace.downloadInstall(download.title, tr.magicplace.mb(download.mb))
            : tr.magicplace.downloadFirstUse(download.title, tr.magicplace.mb(download.mb)),
    );
}

/** Главная кнопка пакета: обновить, установить или открыть — по состоянию и праву на его вид. */
export function PrimaryButton({ entry, actions, open }: { entry: CatalogEntry; actions: PackageActions; open: (() => void) | null }) {
    const busy = actions.busy[entry.id];
    const manage = canManage(entry.kind);
    if (manage && entry.state === 'update') {
        return <Action tone="on" label={tr.magicplace.update} busy={busy === 'update'} disabled={!!busy} onClick={() => void actions.update(entry.id, entry.title)} />;
    }
    // Поставлено, но выключено: окна и виджета у него ещё нет — сначала включить.
    if (isInstalled(entry) && entry.on === false && manage) {
        return <Action tone="on" label={tr.magicplace.switchOn} busy={busy === 'on'} disabled={!!busy} onClick={() => void actions.switchOn(entry)} />;
    }
    if (isInstalled(entry)) return open ? <Action label={tr.magicplace.open} onClick={open} /> : null;
    if (!manage || isBlocked(entry)) return null;
    return <Action tone="on" label={tr.magicplace.install} busy={busy === 'install'} disabled={!!busy} onClick={() => void actions.install(entry)} />;
}

/** Метки пакета в списке: рекомендуем, новое и прежние метки каталога (источник, подпись, состояние). */
function Pills({ entry, engines }: { entry: CatalogEntry; engines: Engines }) {
    return (
        <>
            <span class="pill pill-off">{placeOf(entry.kind, entry.runtime, entry.runs)}</span>
            <PlacePill entry={entry} />
            {recommendedHere(entry) && !isInstalled(entry) && <span class="pill pill-own">{tr.magicplace.recommendedPill}</span>}
            {isNew(entry) && !isInstalled(entry) && <span class="pill pill-agent">{tr.magicplace.newPill}</span>}
            {entry.unlisted && <span class="pill pill-off">{tr.magicplace.hiddenPill}</span>}
            {entryPills(entry, engines).map((pill) => (
                <span key={pill.text} class={`pill ${pill.cls}`}>
                    {pill.text}
                </span>
            ))}
        </>
    );
}

/** Строка списка: картинка, название с метками, строка описания; нажатие — окно пакета. */
function StoreRow({
    entry,
    actions,
    engines,
    onOpen,
    word,
}: {
    entry: CatalogEntry;
    actions: PackageActions;
    engines: Engines;
    onOpen: (id: string) => void;
    /** Своё слово персонажа (п. 65): ставится с ним, отдельно в «Приложениях» его нет. */
    word?: string | null;
}) {
    const panel = usePanel();
    return (
        <div class={`pkg-row${isBlocked(entry) && !isInstalled(entry) ? ' is-dim' : ''}`}>
            <div class="pkg-main">
                <PackageThumb url={entry.preview?.image} />
                <button class="pkg-text text-left cursor-pointer" onClick={() => onOpen(entry.id)}>
                    <span class="pkg-title">
                        {entry.title}
                        <Pills entry={entry} engines={engines} />
                    </span>
                    {entry.description && <span class="t-caption mp-line">{entry.description}</span>}
                    {word && <span class="t-caption">{tr.magicplace.wordLine(word)}</span>}
                    {entry.reason && !isInstalled(entry) && <span class="t-caption">{entry.reason}</span>}
                </button>
                <div class="pkg-actions">
                    <PrimaryButton entry={entry} actions={actions} open={openTarget(entry, panel.go)} />
                </div>
            </div>
            <NoteLine note={actions.notes[entry.id]} />
        </div>
    );
}

/** Ряд витрины — плитки: «Рекомендуем», «Новое». */
function Shelf({ id, title, entries, actions, onOpen }: { id: string; title: string; entries: CatalogEntry[]; actions: PackageActions; onOpen: (id: string) => void }) {
    const panel = usePanel();
    return (
        <section class="card glass card-list" aria-labelledby={`mp-${id}-h`}>
            <div class="card-head">
                <h2 id={`mp-${id}-h`} class="t-title-xs m-0">
                    {title}
                </h2>
            </div>
            <div class="mp-tiles">
                {entries.map((entry) => (
                    <div key={entry.id} class="mp-tile">
                        <button class="mp-tile-open" onClick={() => onOpen(entry.id)}>
                            <PackageThumb url={entry.preview?.image} />
                            <span class="mp-tile-title">{entry.title}</span>
                            {entry.description && <span class="mp-tile-line">{entry.description}</span>}
                            <span class="t-caption">{placeOf(entry.kind, entry.runtime, entry.runs)}</span>
                        </button>
                        <div class="mp-tile-actions">
                            <PrimaryButton entry={entry} actions={actions} open={openTarget(entry, panel.go)} />
                        </div>
                        <NoteLine note={actions.notes[entry.id]} />
                    </div>
                ))}
            </div>
        </section>
    );
}

/** Фильтры над списком: вид — капсулами с числом, лицо, место и состояние — списками; пустой фильтр не рисуется. */
function FiltersBar({ entries, filters, onFilters }: { entries: CatalogEntry[]; filters: Filters; onFilters: (next: Filters) => void }) {
    const set = (change: Partial<Filters>) => onFilters({ ...filters, ...change });
    const count = (test: (entry: CatalogEntry) => boolean) => entries.filter(test).length;
    const kinds = kindFilters().filter((kind) => entries.some((entry) => kindOf(entry) === kind));
    const apps = entries.filter((entry) => kindOf(entry) === 'app');
    const forApps = !filters.kind || filters.kind === 'app';
    const faces = faceFilters().filter((face) => apps.some((entry) => (entry.faces ?? []).includes(face)));
    const places = placeFilters().filter((place) => apps.some((entry) => placeOfEntry(entry) === place));
    const states = stateFilters().filter((state) => entries.some((entry) => stateOf(entry) === state));
    const chip = (active: boolean) => `pill ${active ? 'pill-pick' : 'pill-off'} cursor-pointer min-h-32`;
    const pickKind = (kind: KindFilter | null) => onFilters({ ...filters, kind, ...(kind && kind !== 'app' ? { face: null, place: null } : {}) });
    return (
        <>
            {kinds.length > 1 && (
                <div class="pkg-chips" role="group" aria-label={tr.magicplace.kindGroup}>
                    <button class={chip(!filters.kind)} aria-pressed={!filters.kind} onClick={() => pickKind(null)}>
                        {tr.magicplace.kindAny} <span class="opacity-70">{entries.length}</span>
                    </button>
                    {kinds.map((kind) => (
                        <button key={kind} class={chip(filters.kind === kind)} aria-pressed={filters.kind === kind} onClick={() => pickKind(filters.kind === kind ? null : kind)}>
                            {kindTitle(kind)} <span class="opacity-70">{count((entry) => kindOf(entry) === kind)}</span>
                        </button>
                    ))}
                </div>
            )}
            {(faces.length > 0 || places.length > 1 || states.length > 1 || filtering(filters)) && (
                <div class="pkg-filters">
                    {forApps && faces.length > 0 && (
                        <Dropdown
                            label={tr.magicplace.faceLabel}
                            anyLabel={tr.magicplace.faceAny}
                            value={filters.face}
                            options={faces.map((face) => ({ id: face, label: faceTitle(face), hint: String(count((entry) => (entry.faces ?? []).includes(face))) }))}
                            onPick={(id) => set({ face: id as CatalogFace | null })}
                        />
                    )}
                    {forApps && places.length > 1 && (
                        <Dropdown
                            label={tr.magicplace.placeLabel}
                            anyLabel={tr.magicplace.placeAny}
                            value={filters.place}
                            options={places.map((place) => ({ id: place, label: placeTitle(place), hint: String(count((entry) => placeOfEntry(entry) === place)) }))}
                            onPick={(id) => set({ place: id as CatalogPlace | null })}
                        />
                    )}
                    {states.length > 1 && (
                        <Dropdown
                            label={tr.magicplace.stateLabel}
                            anyLabel={tr.magicplace.stateAny}
                            value={filters.state}
                            options={states.map((state) => ({ id: state, label: stateTitle(state), hint: String(count((entry) => stateOf(entry) === state)) }))}
                            onPick={(id) => set({ state: id as StateFilter | null })}
                        />
                    )}
                    {filtering(filters) && (
                        <button class="t-caption underline cursor-pointer min-h-36" onClick={() => onFilters(NO_FILTERS)}>
                            {tr.magicplace.resetFilters}
                        </button>
                    )}
                </div>
            )}
        </>
    );
}

export function MagicPlaceSection() {
    const panel = usePanel();
    const catalog = useCatalog();
    const engines = useEngines();
    const linked = useLinkedItem('magicplace', STORE_TAB);
    const actions = usePackageActions(() => void catalog.reload());
    const [query, setQuery] = useState('');
    const [filters, setFilters] = useState<Filters>(() => filtersFrom(panel.link.section === 'magicplace' ? panel.link.query : {}));
    const [settings, setSettings] = useState(false);
    const [showBlocked, setShowBlocked] = useState(false);
    const [updatingAll, setUpdatingAll] = useState(false);

    // Ссылка с фильтром («Персонажи» → «Каталог в MagicPlace», «Новый виджет» → «Каталог»): фильтр — её, и когда окно
    // уже открыто.
    const asked = filtersFrom(panel.link.section === 'magicplace' ? panel.link.query : {});
    const askedKey = `${asked.kind}|${asked.face}|${asked.place}|${asked.state}`;
    useEffect(() => {
        if (filtering(asked)) setFilters(asked);
    }, [askedKey]);
    // Окно прочитало каталог — точке в доке и виджету тот же снимок.
    useEffect(() => {
        if (catalog.view) noteStoreChange('', catalog.view);
    }, [catalog.view]);

    const view = catalog.view;
    const open = catalogOpen(view);
    // Слова-активаторы — не отдельные приложения (п. 65): их ставит мастер вместе с персонажем. По фильтру «Слова» — видны
    // (замечание 11 теста 19.11: «Найти слово» в «Слухе» и у персонажа без слова ведёт сюда).
    const all = (view?.packages ?? []).filter((entry) => (open || isInstalled(entry)) && (shownAlone(entry) || filters.kind === 'word'));
    const wordOf = (entry: CatalogEntry): string | null => (entry.kind === 'character' ? wordOfEntry(entry, view?.packages ?? []) : null);
    const searched = all.filter((entry) => matchesQuery(entry, query, filterWords(entry)));
    const found = searched.filter((entry) => passes(entry, filters));
    const here = found.filter(goesHere);
    const blocked = found.filter((entry) => !goesHere(entry));
    const browsing = !query.trim() && !filtering(filters);
    const updates = all.filter((entry) => entry.state === 'update');
    const waiting = all.filter((entry) => !!entry.waits && isInstalled(entry));
    // Поставленное остаётся в «Рекомендуем» и «Новое» — с «Открыть» на своём месте, а не пропадает из ряда (владелец,
    // проверка начисто 18.11, п. 11): только что поставил — тут же и открыл.
    const recommended = all.filter((entry) => recommendedHere(entry) && goesHere(entry));
    const fresh = all
        .filter((entry) => isNew(entry) && goesHere(entry))
        .sort((a, b) => (b.addedAt ?? '').localeCompare(a.addedAt ?? ''));
    const linkedEntry = linked.item ? view?.packages.find((entry) => entry.id === linked.item) : undefined;
    const status = catalogStatus(catalog);
    const manageable = updates.filter((entry) => canManage(entry.kind));

    const updateAll = async () => {
        setUpdatingAll(true);
        try {
            for (const entry of manageable) await actions.update(entry.id, entry.title);
        } finally {
            setUpdatingAll(false);
        }
    };

    return (
        <div class="mp">
            {/* Магазин — вкладка окна «Приложения» (docs/plans/menu-scheme.md): та же шапка, что у «Моих». */}
            <AppsTabs active="magicplace" />
            <div class="mp-top">
                <MagicPlaceMark size={36} />
                <label class="mp-search">
                    <IconSearch size={16} />
                    <input
                        class="mp-search-input"
                        type="search"
                        autocomplete="off"
                        aria-label={tr.magicplace.searchLabel}
                        placeholder={tr.magicplace.searchPlaceholder}
                        value={query}
                        onInput={(event) => setQuery((event.target as HTMLInputElement).value)}
                    />
                    {query && (
                        <button class="mp-search-clear" aria-label={tr.magicplace.clearSearch} onClick={() => setQuery('')}>
                            <IconClose size={14} />
                        </button>
                    )}
                </label>
                <button class="mp-gear" aria-label={tr.magicplace.settings} title={tr.magicplace.settings} onClick={() => setSettings(true)}>
                    <IconGear size={18} />
                </button>
            </div>

            {status && (
                <p class={`t-caption m-0 ${status.tone === 'warning' ? 'c-pill-yandex-fg' : ''}`} role="status">
                    {status.text}
                </p>
            )}
            {feedLines(view).map((line) => (
                <p key={line} class="t-caption m-0 c-pill-yandex-fg" role="status">
                    {line}
                </p>
            ))}

            {browsing && (updates.length > 0 || waiting.length > 0) && (
                <section class="card glass card-list" aria-labelledby="mp-updates-h">
                    <div class="card-head">
                        <h2 id="mp-updates-h" class="t-title-xs m-0">
                            {tr.magicplace.rowUpdates}
                        </h2>
                        {manageable.length > 1 && (
                            <Action tone="on" label={tr.magicplace.updateAll} busy={updatingAll} disabled={updatingAll || Object.keys(actions.busy).length > 0} onClick={() => void updateAll()} />
                        )}
                    </div>
                    {updates.map((entry) => (
                        <div key={entry.id} class="pkg-row">
                            <div class="pkg-main">
                                <PackageThumb url={entry.preview?.image} />
                                <button class="pkg-text text-left cursor-pointer" onClick={() => linked.open(entry.id)}>
                                    <span class="pkg-title">{entry.title}</span>
                                    <span class="t-caption">{tr.magicplace.versionTo(typeof entry.installed === 'string' ? entry.installed : '', entry.version)}</span>
                                </button>
                                <div class="pkg-actions">
                                    <PrimaryButton entry={entry} actions={actions} open={null} />
                                </div>
                            </div>
                            <NoteLine note={actions.notes[entry.id]} />
                        </div>
                    ))}
                    {waiting.map((entry) => (
                        <p key={entry.id} class="t-caption m-0 py-8">
                            {tr.magicplace.waitsGateway(entry.title, entry.waits ?? '')}
                        </p>
                    ))}
                </section>
            )}
            {browsing && recommended.length > 0 && <Shelf id="recommended" title={tr.magicplace.rowRecommended} entries={recommended} actions={actions} onOpen={linked.open} />}
            {browsing && fresh.length > 0 && <Shelf id="new" title={tr.magicplace.rowNew} entries={fresh} actions={actions} onOpen={linked.open} />}

            <section class="card glass card-list" aria-labelledby="mp-all-h">
                <div class="card-head">
                    <h2 id="mp-all-h" class="t-title-xs m-0">
                        {tr.magicplace.rowAll}
                    </h2>
                </div>
                <FiltersBar entries={searched} filters={filters} onFilters={setFilters} />
                {here.map((entry) => (
                    <StoreRow key={entry.id} entry={entry} actions={actions} engines={engines} onOpen={linked.open} word={wordOf(entry)} />
                ))}
                {blocked.length > 0 &&
                    (showBlocked ? (
                        <>
                            {blocked.map((entry) => (
                                <StoreRow key={entry.id} entry={entry} actions={actions} engines={engines} onOpen={linked.open} word={wordOf(entry)} />
                            ))}
                            <button class="mp-more" onClick={() => setShowBlocked(false)}>
                                {tr.magicplace.blockedHide}
                            </button>
                        </>
                    ) : (
                        <button class="mp-more" onClick={() => setShowBlocked(true)}>
                            {tr.magicplace.blockedMore(blocked.length)}
                        </button>
                    ))}
                {view && here.length === 0 && blocked.length === 0 && !status && (
                    <p class="t-caption m-0 py-8">{browsing ? tr.magicplace.emptyCatalog : tr.magicplace.nothingFound}</p>
                )}
            </section>

            {settings && <StoreSettings catalog={catalog} onClose={() => setSettings(false)} />}

            {linkedEntry && (
                <CatalogEntrySheet
                    entry={linkedEntry}
                    actions={actions}
                    engines={engines}
                    all={view?.packages}
                    onClose={linked.close}
                    open={openTarget(linkedEntry, panel.go) ? { label: tr.magicplace.open, go: () => openTarget(linkedEntry, panel.go)?.() } : null}
                >
                    {linkedEntry.agent && <AgentFacts card={linkedEntry.agent} title={linkedEntry.title} />}
                    {downloadLines(linkedEntry).length > 0 && (
                        <WinSection title={tr.magicplace.downloadTitle} group>
                            {downloadLines(linkedEntry).map((line) => (
                                <WinRow key={line} title={line} />
                            ))}
                        </WinSection>
                    )}
                </CatalogEntrySheet>
            )}
        </div>
    );
}
