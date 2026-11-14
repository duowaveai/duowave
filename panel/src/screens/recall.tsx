// «Память» — что персонажи помнят о доме (docs/plans/memory.md, «Управление памятью»; макеты — по ссылке в начале
// плана). Живёт в «Приложениях» (раздел skills), страницей второго слоя: стартовый экран остаётся простым (решение владельца 12.11). Шапка
// (главный переключатель, строка состояния, «Пауза») и вкладки «Обзор», «Дом», «За ночь» (screens/sleep.tsx),
// «Настройки» — остальные вкладки дизайна появятся со своими этапами. Смотреть — право memory.view, менять —
// memory.manage; оба по умолчанию у администратора. Тут же виджет стола и строка «Запомнено» под ответом колонки.
// Файл назван recall.tsx: memory.tsx — карточка «Ресурсы колонки» (её оперативная память и место).

import { useEffect, useState } from 'preact/hooks';

import {
    api,
    can,
    problemOf,
    MemoryChange,
    MemoryEntityCard,
    MemoryEntityItem,
    MemoryEntityKind,
    MemoryFact,
    MemoryLink,
    MemoryOverview,
    MemoryPause,
} from '../api';
import {
    IconChevronLeft,
    IconGear,
    IconGlobe,
    IconHome,
    IconLock,
    IconMemory,
    IconMoon,
    IconPackage,
    IconPause,
    IconPaw,
    IconPin,
    IconPlus,
    IconDoor,
    IconTrash,
    IconUndo,
    IconUser,
} from '../icons';
import { formatDate, formatNumber, locale, tr } from '../i18n';
import { usePoll } from '../poll';
import { useLinkedItem, usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { ask } from '../ui/confirm';
import { Switch } from '../ui/switch';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { MemoryModelSetting, NightTab, SleepSettings } from './sleep';
import { assistant, byMode } from '../words';

export type MemoryTab = 'overview' | 'house' | 'night' | 'settings';

const kindTitles = (): Record<MemoryEntityKind, string> => ({
    household: tr.recall.kindHousehold,
    person: tr.recall.kindPerson,
    pet: tr.recall.kindPet,
    room: tr.recall.kindRoom,
    thing: tr.recall.kindThing,
    place: tr.recall.kindPlace,
    org: tr.recall.kindOrg,
    character: tr.recall.kindCharacter,
});

/** Группы списка «Дом», по порядку. */
const groups = (): Array<{ title: string; kinds: MemoryEntityKind[] }> => [
    { title: tr.recall.groupPeople, kinds: ['person'] },
    { title: tr.recall.groupPets, kinds: ['pet'] },
    { title: tr.recall.groupRooms, kinds: ['room', 'place', 'org'] },
    { title: tr.recall.groupThings, kinds: ['thing'] },
    { title: tr.recall.groupHousehold, kinds: ['household'] },
    { title: tr.recall.groupCharacters, kinds: ['character'] },
];

/** Что можно добавить руками. */
const ADDABLE: MemoryEntityKind[] = ['person', 'pet', 'room', 'place', 'thing', 'org'];

function KindIcon({ kind, size = 16 }: { kind: MemoryEntityKind; size?: number }) {
    switch (kind) {
        case 'household':
            return <IconHome size={size} />;
        case 'pet':
            return <IconPaw size={size} />;
        case 'room':
            return <IconDoor size={size} />;
        case 'place':
        case 'org':
            return <IconGlobe size={size} />;
        case 'thing':
            return <IconPackage size={size} />;
        default:
            return <IconUser size={size} />;
    }
}

/** Аватар: буква имени человека, значок остальных. */
function Avatar({ kind, name }: { kind: MemoryEntityKind; name: string }) {
    return (
        <span class="rc-ava" aria-hidden="true">
            {kind === 'person' || kind === 'character' ? (name.trim()[0] ?? '?').toUpperCase() : <KindIcon kind={kind} />}
        </span>
    );
}

/** «5 КБ», «1,3 МБ», «8 МБ». */
function size(bytes: number): string {
    if (bytes < 1024 * 1024) return tr.recall.kb(Math.max(1, Math.round(bytes / 1024)));
    const mb = bytes / (1024 * 1024);
    return tr.recall.mb(
        Number.isInteger(mb) || mb >= 10 ? String(Math.round(mb)) : formatNumber(mb, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
    );
}

/** «7:00» по часам этого браузера. */
function clock(iso: string): string {
    return formatDate(new Date(iso), { hour: 'numeric', minute: '2-digit' });
}

/** Персонаж по его id: «alice» — Алиса, «пакет:персонаж» — по имени персонажа. */
function characterName(id: string | null, known?: { id: string; title: string }): string {
    if (!id) return '';
    if (known && known.id === id) return known.title;
    if (id === 'alice') return assistant();
    const local = id.includes(':') ? id.split(':').pop() ?? id : id;
    return local.charAt(0).toUpperCase() + local.slice(1);
}

/** Откуда запись — словами: «мимоходом у колонки», «добавлено в пульте». */
function originOf(fact: MemoryFact, stationName: (device: string) => string | null): string {
    const how =
        fact.source === 'panel'
            ? tr.recall.howPanel
            : fact.source === 'voice'
              ? tr.recall.howVoice
              : fact.source === 'model'
                ? tr.recall.howModel
                : fact.source === 'reflection'
                  ? tr.recall.howReflection
                  : fact.source === 'package'
                    ? tr.recall.howPackage
                    : tr.recall.howConversation;
    if (fact.source === 'panel') return how;
    const station = fact.origin.station ? stationName(fact.origin.station) : null;
    const where =
        fact.origin.channel === 'station'
            ? station
                ? tr.recall.whereStationNamed(station)
                : byMode(tr.recall.whereStation, tr.recall.whereVoice)
            : fact.origin.channel === 'panel'
              ? tr.recall.wherePanel
              : fact.origin.channel === 'telegram'
                ? tr.recall.whereTelegram
                : '';
    return `${how}${where}`;
}

function whoKnows(fact: MemoryFact): string {
    switch (fact.visibility) {
        case 'person':
            return tr.recall.knowsPerson;
        case 'people':
            return tr.recall.knowsPeople;
        case 'character':
            return tr.recall.knowsCharacter;
        case 'world':
            return tr.recall.knowsWorld;
        default:
            return tr.recall.knowsHouse;
    }
}

function certainty(confidence: number): string {
    return confidence >= 0.95 ? tr.recall.certain : confidence >= 0.7 ? tr.recall.likely : tr.recall.guess;
}

/** Имя колонки по метке устройства, если пульт её знает. */
function useStationName(): (device: string) => string | null {
    const panel = usePanel();
    return (device) => (panel.current?.device === device ? (panel.current?.name ?? null) : null);
}

/** Строка состояния под переключателем. */
function stateLine(overview: MemoryOverview | null): string {
    if (!overview) return tr.recall.reading;
    const { state, counts, bytes } = overview;
    if (!state.enabled) return tr.recall.stateOff;
    const parts = [tr.recall.homeRecords(counts.home), tr.recall.aboutSize(size(bytes))];
    if (state.paused) parts.push(state.pausedUntil ? tr.recall.pausedUntil(clock(state.pausedUntil)) : tr.recall.pausedManual);
    return parts.join(' · ');
}

/** Вкладка памяти, открытая карточка и ночь — в адресе «Приложений»: `#/skills/memory/<вкладка>?e=<id>`, `…/night?n=<id>`. */
function useMemoryPlace(): {
    tab: MemoryTab;
    entity: string | null;
    night: string | null;
    go: (tab: MemoryTab, entity?: string) => void;
    openNight: (id?: string) => void;
} {
    const linked = useLinkedItem('skills', 'memory');
    const tab: MemoryTab = linked.item === 'house' || linked.item === 'night' || linked.item === 'settings' ? linked.item : 'overview';
    return {
        tab,
        entity: linked.query.e || null,
        night: linked.query.n || null,
        go: (next, entity) => linked.open(next, entity ? { e: entity } : {}),
        openNight: (id) => linked.open('night', id ? { n: id } : {}),
    };
}

/** «Приложения» → «Память». */
export function MemoryScreen() {
    const panel = usePanel();
    const place = useMemoryPlace();
    const tab = place.tab;
    const setTab = (next: MemoryTab) => place.go(next);
    const [overview, setOverview] = useState<MemoryOverview | null>(null);
    const [error, setError] = useState<string | null>(null);
    const reread = usePoll({
        every: 30_000,
        version: panel.version,
        read: (signal) =>
            api
                .memory(signal)
                .then((fresh) => {
                    if (signal.aborted) return;
                    setOverview(fresh);
                    setError(null);
                })
                .catch((e) => {
                    if (!signal.aborted) setError(problemOf(e));
                }),
    });
    const tabs: Array<TabEntry<MemoryTab>> = [
        { id: 'overview', label: tr.recall.tabOverview, icon: <IconMemory size={16} /> },
        { id: 'house', label: tr.recall.tabHouse, icon: <IconHome size={16} /> },
        { id: 'night', label: tr.recall.tabNight, icon: <IconMoon size={16} /> },
        { id: 'settings', label: tr.recall.tabSettings, icon: <IconGear size={16} /> },
    ];
    const shown: MemoryTab = tabs.some((entry) => entry.id === tab) ? tab : 'overview';
    return (
        <>
            <MemoryHead overview={overview} error={error} onChanged={reread} />
            <SectionTabs label={tr.recall.tabsLabel} tabs={tabs} tab={shown} onPick={setTab} />
            {shown === 'overview' && <OverviewTab overview={overview} onChanged={reread} />}
            {shown === 'house' && <HouseTab />}
            {shown === 'night' && (
                <NightTab night={place.night} onOpen={(id) => place.openNight(id)} onBack={() => place.openNight()} onSettings={() => place.go('settings')} />
            )}
            {shown === 'settings' && <SettingsTab overview={overview} onChanged={reread} />}
        </>
    );
}

/** Шапка: включена ли память, что в ней, пауза. Видна на любой вкладке. */
function MemoryHead({ overview, error, onChanged }: { overview: MemoryOverview | null; error: string | null; onChanged: () => void }) {
    const manage = can('memory.manage');
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const state = overview?.state;
    const run = (job: () => Promise<unknown>) => {
        setBusy(true);
        setProblem(null);
        job()
            .then(onChanged)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const toggle = async (on: boolean) => {
        if (!on) {
            const sure = await ask({
                title: tr.recall.turnOffTitle,
                text: tr.recall.turnOffText,
                ok: tr.recall.turnOffOk,
            });
            if (!sure) return;
        }
        run(() => api.setMemoryState({ enabled: on }));
    };
    return (
        <section class="card glass rc-head" aria-label={tr.recall.headLabel}>
            <div class="rc-head-row">
                <label class="w-toggle flex-1 min-w-0">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{!state ? tr.recall.headTitle : state.enabled ? tr.recall.headOn : tr.recall.headOff}</span>
                        <span class="w-cap">{stateLine(overview)}</span>
                    </span>
                    <Switch on={!!state?.enabled} label={tr.recall.headTitle} disabled={!manage || !state || busy} onChange={(on) => void toggle(on)} />
                </label>
                {state?.enabled && <PauseControl paused={state.paused} busy={busy} onRun={run} />}
            </div>
            {(problem ?? error) && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {problem ?? error}
                </p>
            )}
        </section>
    );
}

/** «Пауза»: на час, до утра или пока не включу — то же, что «не запоминай» голосом. */
function PauseControl({ paused, busy, onRun }: { paused: boolean; busy: boolean; onRun: (job: () => Promise<unknown>) => void }) {
    const [open, setOpen] = useState(false);
    if (paused) {
        return (
            <button class="w-btn" disabled={busy} onClick={() => onRun(() => api.resumeMemory())}>
                {tr.recall.resume}
            </button>
        );
    }
    const pick = (span: MemoryPause) => {
        setOpen(false);
        onRun(() => api.pauseMemory(span));
    };
    return (
        <div class="rc-pause">
            <button class="w-btn" aria-haspopup="menu" aria-expanded={open} disabled={busy} onClick={() => setOpen(!open)}>
                <IconPause size={14} />
                {tr.recall.pause}
            </button>
            {open && (
                <div class="menu rc-menu" role="menu" aria-label={tr.recall.dontRemember}>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick('hour')}>
                        {tr.recall.forHour}
                    </button>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick('morning')}>
                        {tr.recall.untilMorning}
                    </button>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick('manual')}>
                        {tr.recall.untilOn}
                    </button>
                    <p class="w-cap rc-menu-note">{tr.recall.pauseNote}</p>
                </div>
            )}
        </div>
    );
}

/** «Обзор»: сколько чего, «Новое», «Глазами персонажа», что уходит наружу. */
function OverviewTab({ overview, onChanged }: { overview: MemoryOverview | null; onChanged: () => void }) {
    if (!overview) return <p class="w-cap">{tr.recall.reading}</p>;
    const { counts, bytes, limitBytes } = overview;
    const share = Math.min(100, Math.round((bytes / Math.max(1, limitBytes)) * 1000) / 10);
    return (
        <>
            <section class="card glass card-list" aria-label={tr.recall.whatsInMemory}>
                <div class="rc-stats">
                    <div class="rc-stat rc-home">
                        <span class="rc-stat-label">
                            <IconHome size={14} />
                            {tr.recall.statHome}
                        </span>
                        <b>{counts.home}</b>
                        <span class="w-cap">{tr.recall.statHomeAbout}</span>
                    </div>
                    <div class="rc-stat rc-pers">
                        <span class="rc-stat-label">
                            <IconMemory size={14} />
                            {tr.recall.statPersonal}
                        </span>
                        <b>{counts.personal}</b>
                        <span class="w-cap">{tr.recall.statPersonalAbout}</span>
                    </div>
                    <div class="rc-stat">
                        <span class="rc-stat-label">{tr.recall.statTalks}</span>
                        <b>{counts.episodes}</b>
                        <span class="w-cap">{tr.recall.statTalksAbout}</span>
                    </div>
                </div>
                <div class="rc-size">
                    <span class="w-cap">
                        {tr.recall.usedBefore} <b>{size(bytes)}</b>
                        {tr.recall.usedOf(size(limitBytes))}
                    </span>
                    <span class="rc-bar" role="img" aria-label={tr.recall.usedPercent(formatNumber(share))}>
                        <i style={{ width: `${Math.max(share, bytes > 0 ? 1 : 0)}%` }} />
                    </span>
                </div>
            </section>
            <RecentCard overview={overview} onChanged={onChanged} />
            <EyesCard overview={overview} />
            <section class="card glass card-list" aria-labelledby="rc-out-h">
                <div class="section-head">
                    <h2 id="rc-out-h" class="t-title-xs m-0">
                        {tr.recall.outTitle}
                    </h2>
                </div>
                <p class="w-cap m-0">
                    {tr.recall.outText}
                </p>
            </section>
        </>
    );
}

/** «Новое»: последние записи — что, кто сказал, где, когда; у каждой — «Отменить». */
function RecentCard({ overview, onChanged }: { overview: MemoryOverview; onChanged: () => void }) {
    const panel = usePanel();
    const stationName = useStationName();
    const manage = can('memory.manage');
    const [problem, setProblem] = useState<string | null>(null);
    const undo = (changeId: string) => {
        setProblem(null);
        api.undoMemoryChange(changeId)
            .then(onChanged)
            .catch((e) => setProblem(problemOf(e)));
    };
    return (
        <section class="card glass card-list" aria-labelledby="rc-new-h">
            <div class="section-head">
                <h2 id="rc-new-h" class="t-title-xs m-0">
                    {tr.recall.newTitle}
                </h2>
                <button class="rc-link" onClick={() => panel.go('skills', 'memory', 'house')}>
                    {tr.recall.wholeHouse}
                </button>
            </div>
            {overview.recent.length === 0 ? (
                <p class="w-cap m-0">
                    {byMode(tr.recall.emptyStation, tr.recall.emptyComputer)}
                </p>
            ) : (
                <ul class="rc-list">
                    {overview.recent.map((item) => (
                        <li key={item.id} class="rc-item">
                            <span class="w-cap rc-time">{timeAgo(item.createdAt)}</span>
                            <span class="min-w-0 flex flex-col">
                                <span class={item.validTo ? 'rc-item-text rc-closed' : 'rc-item-text'}>{item.text}</span>
                                <span class="rc-item-meta">
                                    <span class={`rc-chip rc-${item.layer === 'personal' ? 'pers' : 'home'}`}>
                                        {item.layer === 'personal'
                                            ? tr.recall.personal(characterName(item.visibilityOwner))
                                            : item.subjectKind === 'household'
                                              ? tr.recall.house
                                              : tr.recall.houseAbout(item.subjectName)}
                                    </span>
                                    <span>{originOf(item, stationName)}</span>
                                    {item.validTo && <span>{tr.recall.closed}</span>}
                                </span>
                            </span>
                            {manage && item.changeId && !item.validTo ? (
                                <button class="w-btn rc-undo" onClick={() => undo(item.changeId!)}>
                                    <IconUndo size={13} />
                                    {tr.recall.undo}
                                </button>
                            ) : (
                                <span />
                            )}
                        </li>
                    ))}
                </ul>
            )}
            {problem && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {problem}
                </p>
            )}
        </section>
    );
}

/** «Глазами персонажа»: ровно то, что всегда стоит в его подсказке. */
function EyesCard({ overview }: { overview: MemoryOverview }) {
    const { eyes } = overview;
    const share = Math.min(100, Math.round((eyes.tokens / Math.max(1, eyes.maxTokens)) * 100));
    return (
        <section class="card glass card-list" aria-labelledby="rc-eyes-h">
            <div class="section-head">
                <h2 id="rc-eyes-h" class="t-title-xs m-0">
                    {tr.recall.eyesTitle}
                </h2>
                <span class="pill pill-off">{eyes.character.title}</span>
            </div>
            <p class="w-cap m-0">{tr.recall.eyesAbout}</p>
            {eyes.items.length === 0 ? (
                <p class="w-cap m-0">{tr.recall.eyesEmpty}</p>
            ) : (
                <ul class="rc-eyes">
                    {eyes.items.map((item) => (
                        <li key={item.id}>
                            {item.pinned ? <IconPin size={13} /> : <span class={`rc-dot rc-${item.layer === 'personal' ? 'pers' : 'home'}`} />}
                            <span>{item.text}</span>
                        </li>
                    ))}
                </ul>
            )}
            <span class="rc-bar rc-bar-ink" role="img" aria-label={tr.recall.eyesPercent(share)}>
                <i style={{ width: `${share}%` }} />
            </span>
            <span class="w-cap">
                {tr.recall.eyesCount(eyes.items.length, eyes.maxItems, eyes.tokens, eyes.maxTokens)}
            </span>
        </section>
    );
}

/** «Дом»: список людей, питомцев, комнат и мест; по нажатию — карточка. */
function HouseTab() {
    const place = useMemoryPlace();
    if (place.entity) return <EntityView id={place.entity} onBack={() => place.go('house')} onOpen={(id) => place.go('house', id)} />;
    return <EntityList onOpen={(id) => place.go('house', id)} />;
}

function EntityList({ onOpen }: { onOpen: (id: string) => void }) {
    const panel = usePanel();
    const manage = can('memory.manage');
    const [list, setList] = useState<MemoryEntityItem[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [query, setQuery] = useState('');
    const [adding, setAdding] = useState(false);
    const reread = usePoll({
        every: 60_000,
        version: panel.version,
        read: (signal) =>
            api
                .memoryEntities(signal)
                .then((fresh) => {
                    if (!signal.aborted) {
                        setList(fresh);
                        setError(null);
                    }
                })
                .catch((e) => {
                    if (!signal.aborted) setError(problemOf(e));
                }),
    });
    const q = query.trim().toLowerCase().replace(/ё/g, 'е');
    const found = (list ?? []).filter(
        (entity) => !q || [entity.name, ...entity.aliases, entity.note ?? ''].some((text) => text.toLowerCase().replace(/ё/g, 'е').includes(q)),
    );
    return (
        <section class="card glass card-list" aria-labelledby="rc-house-h">
            <div class="section-head">
                <h2 id="rc-house-h" class="t-title-xs m-0">
                    {tr.recall.houseTitle}
                </h2>
                {manage && !adding && (
                    <button class="w-btn" onClick={() => setAdding(true)}>
                        <IconPlus size={14} />
                        {tr.recall.add}
                    </button>
                )}
            </div>
            {adding && <AddEntity onDone={(id) => (setAdding(false), reread(), id && onOpen(id))} />}
            <input
                class="field rc-search"
                type="search"
                placeholder={tr.recall.searchHouse}
                aria-label={tr.recall.searchHouse}
                value={query}
                onInput={(event) => setQuery((event.target as HTMLInputElement).value)}
            />
            {error && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
            {!list ? (
                <p class="w-cap m-0">{tr.recall.reading}</p>
            ) : list.length === 0 || (list.length === 1 && list[0].kind === 'household' && list[0].facts === 0) ? (
                <p class="w-cap m-0">
                    {byMode(tr.recall.houseEmptyStation, tr.recall.houseEmptyComputer)}
                </p>
            ) : (
                groups().map((group) => {
                    const members = found.filter((entity) => group.kinds.includes(entity.kind) && (entity.kind !== 'household' || entity.facts > 0));
                    if (members.length === 0) return null;
                    return (
                        <div key={group.title} class="flex flex-col">
                            <span class="rc-group">{group.title}</span>
                            {members.map((entity) => (
                                <button key={entity.id} class="rc-entity" onClick={() => onOpen(entity.id)}>
                                    <Avatar kind={entity.kind} name={entity.name} />
                                    <span class="min-w-0 flex flex-col">
                                        <span class="w-strong">{entity.name}</span>
                                        <span class="w-cap rc-ellipsis">{entity.aliases.length ? entity.aliases.join(', ') : (entity.note ?? kindTitles()[entity.kind])}</span>
                                    </span>
                                    <span class="w-cap">{entity.facts}</span>
                                </button>
                            ))}
                        </div>
                    );
                })
            )}
        </section>
    );
}

function AddEntity({ onDone }: { onDone: (id?: string) => void }) {
    const [kind, setKind] = useState<MemoryEntityKind>('person');
    const [name, setName] = useState('');
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const add = (event: Event) => {
        event.preventDefault();
        if (!name.trim()) return;
        setBusy(true);
        api.addMemoryEntity(kind, name.trim())
            .then((entity) => onDone(entity.id))
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    return (
        <form class="rc-form" onSubmit={add}>
            <select class="field rc-kind" aria-label={tr.recall.whoIsIt} value={kind} onChange={(event) => setKind((event.target as HTMLSelectElement).value as MemoryEntityKind)}>
                {ADDABLE.map((value) => (
                    <option key={value} value={value}>
                        {kindTitles()[value]}
                    </option>
                ))}
            </select>
            <input class="field flex-1 min-w-0" placeholder={tr.recall.namePlaceholder} aria-label={tr.recall.nameLabel} value={name} onInput={(event) => setName((event.target as HTMLInputElement).value)} />
            <button class="w-btn w-btn-ink" type="submit" disabled={busy || !name.trim()}>
                {tr.recall.add}
            </button>
            <button class="w-btn" type="button" onClick={() => onDone()}>
                {tr.recall.cancel}
            </button>
            {problem && (
                <p class="w-cap c-dot-offline m-0 rc-form-note" role="alert">
                    {problem}
                </p>
            )}
        </form>
    );
}

/** Карточка: другие имена, что сейчас верно, связи «кто с кем», история закрытого. */
function EntityView({ id, onBack, onOpen }: { id: string; onBack: () => void; onOpen: (id: string) => void }) {
    const panel = usePanel();
    const manage = can('memory.manage');
    const [card, setCard] = useState<MemoryEntityCard | null>(null);
    const [others, setOthers] = useState<MemoryEntityItem[]>([]);
    const [error, setError] = useState<string | null>(null);
    const [adding, setAdding] = useState(false);
    const [merging, setMerging] = useState(false);
    const reread = usePoll({
        deps: [id],
        every: 60_000,
        version: panel.version,
        read: (signal) =>
            api
                .memoryEntity(id, signal)
                .then((fresh) => {
                    if (!signal.aborted) {
                        setCard(fresh);
                        setError(null);
                    }
                })
                .catch((e) => {
                    if (!signal.aborted) setError(problemOf(e));
                }),
    });
    useEffect(() => {
        if (!merging) return;
        const control = new AbortController();
        api.memoryEntities(control.signal)
            .then(setOthers)
            .catch(() => setOthers([]));
        return () => control.abort();
    }, [merging]);
    const act = (job: () => Promise<unknown>, after?: () => void) => {
        setError(null);
        job()
            .then(() => (after ? after() : reread()))
            .catch((e) => setError(problemOf(e)));
    };
    const back = (
        <button class="deeper-back" onClick={onBack}>
            <IconChevronLeft size={16} />
            {tr.recall.wholeHouse}
        </button>
    );
    if (!card) {
        return (
            <>
                {back}
                <p class={error ? 'w-cap c-dot-offline' : 'w-cap'} role={error ? 'alert' : undefined}>
                    {error ?? tr.recall.reading}
                </p>
            </>
        );
    }
    const { entity } = card;
    const household = entity.kind === 'household';
    const remove = async () => {
        const sure = await ask({
            title: tr.recall.eraseTitle(entity.name),
            text: tr.recall.eraseText,
            ok: tr.recall.eraseOk,
            danger: true,
        });
        if (sure) act(() => api.deleteMemoryEntity(entity.id), onBack);
    };
    const merge = async (other: MemoryEntityItem) => {
        const sure = await ask({
            title: tr.recall.mergeTitle(other.name, entity.name),
            text: tr.recall.mergeText(other.name),
            ok: tr.recall.mergeOk,
        });
        if (sure) act(() => api.mergeMemoryEntity(other.id, entity.id), () => (setMerging(false), reread()));
    };
    return (
        <>
            {back}
            <section class="card glass card-list" aria-labelledby="rc-card-h">
                <div class="rc-card-head">
                    <Avatar kind={entity.kind} name={entity.name} />
                    <div class="flex-1 min-w-0 flex flex-col">
                        <h2 id="rc-card-h" class="t-title-xs m-0">
                            {entity.name}
                        </h2>
                        <span class="w-cap">{kindTitles()[entity.kind]}</span>
                    </div>
                    {manage && !household && (
                        <div class="rc-head-actions">
                            <button class="w-btn" onClick={() => setMerging(!merging)} aria-expanded={merging}>
                                {tr.recall.mergeWith}
                            </button>
                            <button class="w-btn rc-danger" onClick={() => void remove()} aria-label={tr.recall.eraseLabel(entity.name)}>
                                <IconTrash size={14} />
                            </button>
                        </div>
                    )}
                </div>
                {!household && <Aliases card={card} manage={manage} onChanged={reread} />}
                {merging && (
                    <div class="rc-merge">
                        <span class="w-cap">{tr.recall.mergeAsk}</span>
                        <div class="rc-chips">
                            {others
                                .filter((other) => other.id !== entity.id && other.kind !== 'household')
                                .map((other) => (
                                    <button key={other.id} class="rc-chip rc-chip-btn" onClick={() => void merge(other)}>
                                        {other.name}
                                    </button>
                                ))}
                        </div>
                    </div>
                )}
                {error && (
                    <p class="w-cap c-dot-offline m-0" role="alert">
                        {error}
                    </p>
                )}
            </section>

            <section class="card glass card-list" aria-labelledby="rc-now-h">
                <div class="section-head">
                    <h2 id="rc-now-h" class="t-title-xs m-0">
                        {tr.recall.now}
                    </h2>
                    {manage && !adding && (
                        <button class="w-btn" onClick={() => setAdding(true)}>
                            <IconPlus size={14} />
                            {tr.recall.fact}
                        </button>
                    )}
                </div>
                {adding && <AddFact subjectId={entity.id} onDone={() => (setAdding(false), reread())} />}
                {card.current.length === 0 ? (
                    <p class="w-cap m-0">{tr.recall.nothingKnown}</p>
                ) : (
                    <ul class="rc-list">
                        {card.current.map((fact) => (
                            <FactRow key={fact.id} fact={fact} manage={manage} onAct={act} />
                        ))}
                    </ul>
                )}
            </section>

            {card.links.length > 0 && (
                <section class="card glass card-list" aria-labelledby="rc-links-h">
                    <div class="section-head">
                        <h2 id="rc-links-h" class="t-title-xs m-0">
                            {tr.recall.links}
                        </h2>
                    </div>
                    <LinksGraph name={entity.name} links={card.links} onOpen={onOpen} />
                    <div class="rc-chips">
                        {card.links.map((link) => (
                            <button key={link.factId} class={link.closed ? 'rc-chip rc-chip-btn rc-closed' : 'rc-chip rc-chip-btn'} onClick={() => onOpen(link.otherId)}>
                                {link.relation ? `${link.relation} · ` : ''}
                                {link.otherName}
                            </button>
                        ))}
                    </div>
                </section>
            )}

            {card.closed.length > 0 && (
                <section class="card glass card-list" aria-labelledby="rc-history-h">
                    <div class="section-head">
                        <h2 id="rc-history-h" class="t-title-xs m-0">
                            {tr.recall.history}
                        </h2>
                    </div>
                    <ul class="rc-list">
                        {card.closed.map((fact) => (
                            <li key={fact.id} class="rc-item rc-item-history">
                                <span class="min-w-0 flex flex-col">
                                    <span class="rc-item-text rc-closed">{fact.text}</span>
                                    <span class="rc-item-meta">
                                        {tr.recall.until(fact.validTo ? new Date(fact.validTo).toLocaleDateString(locale()) : '—')}
                                        {fact.closedReason ? ` · ${fact.closedReason}` : ''}
                                    </span>
                                </span>
                                {manage ? (
                                    <button class="w-btn" onClick={() => act(() => api.reopenMemoryFact(fact.id))}>
                                        {tr.recall.restore}
                                    </button>
                                ) : (
                                    <span />
                                )}
                            </li>
                        ))}
                    </ul>
                </section>
            )}
        </>
    );
}

/** Другие имена: «Мария», «сестра Пети» — по ним персонаж узнаёт, о ком речь. */
function Aliases({ card, manage, onChanged }: { card: MemoryEntityCard; manage: boolean; onChanged: () => void }) {
    const [adding, setAdding] = useState(false);
    const [name, setName] = useState('');
    const [problem, setProblem] = useState<string | null>(null);
    const save = (aliases: string[]) => {
        setProblem(null);
        api.updateMemoryEntity(card.entity.id, { aliases })
            .then(() => (setAdding(false), setName(''), onChanged()))
            .catch((e) => setProblem(problemOf(e)));
    };
    const { aliases } = card.entity;
    if (!manage && aliases.length === 0) return null;
    return (
        <div class="flex flex-col gap-4">
            <div class="rc-chips" aria-label={tr.recall.otherNames}>
                {aliases.map((alias) => (
                    <span key={alias} class="rc-chip">
                        {alias}
                        {manage && (
                            <button class="rc-chip-x" aria-label={tr.recall.removeName(alias)} onClick={() => save(aliases.filter((a) => a !== alias))}>
                                ×
                            </button>
                        )}
                    </span>
                ))}
                {manage &&
                    (adding ? (
                        <form
                            class="rc-form rc-form-inline"
                            onSubmit={(event) => {
                                event.preventDefault();
                                if (name.trim()) save([...aliases, name.trim()]);
                            }}
                        >
                            <input class="field" placeholder={tr.recall.otherName} aria-label={tr.recall.otherName} value={name} onInput={(event) => setName((event.target as HTMLInputElement).value)} />
                            <button class="w-btn" type="submit" disabled={!name.trim()}>
                                {tr.recall.add}
                            </button>
                        </form>
                    ) : (
                        <button class="rc-chip rc-chip-btn rc-chip-add" onClick={() => setAdding(true)}>
                            {tr.recall.plusName}
                        </button>
                    ))}
            </div>
            {problem && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {problem}
                </p>
            )}
        </div>
    );
}

function AddFact({ subjectId, onDone }: { subjectId: string; onDone: () => void }) {
    const [text, setText] = useState('');
    const [pinned, setPinned] = useState(false);
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const add = (event: Event) => {
        event.preventDefault();
        if (!text.trim()) return;
        setBusy(true);
        api.addMemoryFact(subjectId, text.trim(), pinned)
            .then(onDone)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    return (
        <form class="rc-form" onSubmit={add}>
            <input class="field flex-1 min-w-0" placeholder={tr.recall.factPlaceholder} aria-label={tr.recall.fact} value={text} onInput={(event) => setText((event.target as HTMLInputElement).value)} />
            <label class="rc-check">
                <input type="checkbox" checked={pinned} onChange={(event) => setPinned((event.target as HTMLInputElement).checked)} />
                {tr.recall.alwaysInPrompt}
            </label>
            <button class="w-btn w-btn-ink" type="submit" disabled={busy || !text.trim()}>
                {tr.recall.remember}
            </button>
            <button class="w-btn" type="button" onClick={onDone}>
                {tr.recall.cancel}
            </button>
            {problem && (
                <p class="w-cap c-dot-offline m-0 rc-form-note" role="alert">
                    {problem}
                </p>
            )}
        </form>
    );
}

/** Факт: по нажатию — откуда он, кто знает, и что с ним сделать. */
function FactRow({ fact, manage, onAct }: { fact: MemoryFact; manage: boolean; onAct: (job: () => Promise<unknown>) => void }) {
    const stationName = useStationName();
    const [open, setOpen] = useState(false);
    const [editing, setEditing] = useState(false);
    const [text, setText] = useState(fact.text);
    const erase = async () => {
        const sure = await ask({ title: tr.recall.eraseFactTitle, text: tr.recall.eraseFactText(fact.text), ok: tr.recall.eraseOk, danger: true });
        if (sure) onAct(() => api.eraseMemoryFact(fact.id));
    };
    return (
        <li class={open ? 'rc-fact rc-fact-open' : 'rc-fact'}>
            {editing ? (
                <form
                    class="rc-form"
                    onSubmit={(event) => {
                        event.preventDefault();
                        if (!text.trim()) return;
                        setEditing(false);
                        onAct(() => api.updateMemoryFact(fact.id, { text: text.trim() }));
                    }}
                >
                    <input class="field flex-1 min-w-0" aria-label={tr.recall.fact} value={text} onInput={(event) => setText((event.target as HTMLInputElement).value)} />
                    <button class="w-btn w-btn-ink" type="submit" disabled={!text.trim()}>
                        {tr.recall.save}
                    </button>
                    <button class="w-btn" type="button" onClick={() => (setEditing(false), setText(fact.text))}>
                        {tr.recall.cancel}
                    </button>
                </form>
            ) : (
                <button class="rc-fact-main" aria-expanded={open} onClick={() => setOpen(!open)}>
                    <span class="rc-item-text">
                        {fact.pinned && <IconPin size={13} />}
                        {fact.text}
                    </span>
                    <span class="rc-item-meta">
                        {originOf(fact, stationName)} · {timeAgo(fact.createdAt)}
                        {fact.kind === 'insight' && <span class="rc-chip">{tr.recall.insight}</span>}
                    </span>
                </button>
            )}
            {open && !editing && (
                <div class="rc-fact-more">
                    <dl class="rc-kv">
                        <dt>{tr.recall.certainty}</dt>
                        <dd>{certainty(fact.confidence)}</dd>
                        <dt>{tr.recall.whoKnows}</dt>
                        <dd>
                            {whoKnows(fact)}
                            {fact.origin.heardBy ? tr.recall.heardBy(characterName(fact.origin.heardBy)) : ''}
                        </dd>
                        <dt>{tr.recall.whoSaid}</dt>
                        <dd>
                            {fact.origin.saidByKind === 'household'
                                ? byMode(tr.recall.saidHouseStation, tr.recall.saidHouseComputer)
                                : fact.origin.saidByKind === 'user'
                                  ? tr.recall.saidUser
                                  : fact.origin.saidByKind === 'contact'
                                    ? tr.recall.saidContact
                                    : tr.recall.saidGateway}
                        </dd>
                        <dt>{tr.recall.inPrompt}</dt>
                        <dd>{fact.pinned ? tr.recall.pinnedAlways : tr.recall.byImportance}</dd>
                    </dl>
                    {manage && (
                        <div class="rc-fact-actions">
                            <button class="w-btn" onClick={() => setEditing(true)}>
                                {tr.recall.fix}
                            </button>
                            <button class="w-btn" onClick={() => onAct(() => api.updateMemoryFact(fact.id, { pinned: !fact.pinned }))}>
                                <IconPin size={13} />
                                {fact.pinned ? tr.recall.unpin : tr.recall.pin}
                            </button>
                            <button class="w-btn" onClick={() => onAct(() => api.closeMemoryFact(fact.id))}>
                                {tr.recall.noLongerTrue}
                            </button>
                            <button class="w-btn rc-danger" onClick={() => void erase()}>
                                <IconTrash size={13} />
                                {tr.recall.eraseOk}
                            </button>
                        </div>
                    )}
                </div>
            )}
        </li>
    );
}

/** «Кто с кем»: он в середине, связанные — вокруг; закрытые связи — пунктиром. */
function LinksGraph({ name, links, onOpen }: { name: string; links: MemoryLink[]; onOpen: (id: string) => void }) {
    const shown = links.slice(0, 8);
    const width = 360;
    const height = 220;
    const [cx, cy] = [width / 2, height / 2];
    const nodes = shown.map((link, at) => {
        const angle = (2 * Math.PI * at) / shown.length - Math.PI / 2;
        return { link, x: cx + 128 * Math.cos(angle), y: cy + 76 * Math.sin(angle) };
    });
    const pill = (text: string) => Math.min(120, 18 + text.length * 7);
    return (
        <svg class="rc-graph" viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
            {nodes.map(({ link, x, y }) => (
                <line key={`l-${link.factId}`} x1={cx} y1={cy} x2={x} y2={y} class={link.closed ? 'rc-g-line rc-g-closed' : 'rc-g-line'} />
            ))}
            {nodes.map(({ link, x, y }) =>
                link.relation ? (
                    <text key={`r-${link.factId}`} x={(cx + x) / 2} y={(cy + y) / 2 - 3} class="rc-g-rel" text-anchor="middle">
                        {link.relation.length > 14 ? `${link.relation.slice(0, 13)}…` : link.relation}
                    </text>
                ) : null,
            )}
            {nodes.map(({ link, x, y }) => {
                const label = link.otherName.length > 15 ? `${link.otherName.slice(0, 14)}…` : link.otherName;
                const w = pill(label);
                return (
                    <g key={`n-${link.factId}`} class={link.closed ? 'rc-g-node rc-g-closed' : 'rc-g-node'} onClick={() => onOpen(link.otherId)}>
                        <rect x={x - w / 2} y={y - 12} width={w} height={24} rx={12} />
                        <text x={x} y={y + 4} text-anchor="middle">
                            {label}
                        </text>
                    </g>
                );
            })}
            <circle cx={cx} cy={cy} r={28} class="rc-g-center" />
            <text x={cx} y={cy + 4} text-anchor="middle" class="rc-g-center-text">
                {name.length > 9 ? `${name.slice(0, 8)}…` : name}
            </text>
        </svg>
    );
}

/** «Настройки»: всё управление памятью — то, что есть на этом этапе. */
function SettingsTab({ overview, onChanged }: { overview: MemoryOverview | null; onChanged: () => void }) {
    const manage = can('memory.manage');
    const place = useMemoryPlace();
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const state = overview?.state;
    const set = (change: { enabled?: boolean; inPassing?: boolean }) => {
        setBusy(true);
        setProblem(null);
        api.setMemoryState(change)
            .then(onChanged)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const toggleMemory = async (on: boolean) => {
        if (!on) {
            const sure = await ask({
                title: tr.recall.turnOffTitle,
                text: tr.recall.turnOffText,
                ok: tr.recall.turnOffOk,
            });
            if (!sure) return;
        }
        set({ enabled: on });
    };
    return (
        <>
            <section class="card glass card-list" aria-labelledby="rc-set-h">
                <div class="section-head">
                    <h2 id="rc-set-h" class="t-title-xs m-0">
                        {tr.recall.settingsMemory}
                    </h2>
                </div>
                <div class="rc-toggles">
                <label class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{tr.recall.settingsMemory}</span>
                        <span class="w-cap">{tr.recall.memoryAbout}</span>
                    </span>
                    <Switch on={!!state?.enabled} label={tr.recall.settingsMemory} disabled={!manage || !state || busy} onChange={(on) => void toggleMemory(on)} />
                </label>
                <label class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{tr.recall.inPassing}</span>
                        <span class="w-cap">{tr.recall.inPassingAbout}</span>
                    </span>
                    <Switch on={!!state?.inPassing} label={tr.recall.inPassing} disabled={!manage || !state || busy || !state?.enabled} onChange={(on) => set({ inPassing: on })} />
                </label>
                <MemoryModelSetting />
                </div>
                {problem && (
                    <p class="w-cap c-dot-offline m-0" role="alert">
                        {problem}
                    </p>
                )}
                {!manage && <p class="w-cap m-0">{tr.recall.needRight}</p>}
            </section>
            <SleepSettings onNight={(id) => place.openNight(id)} />
            <section class="card glass card-list" aria-labelledby="rc-keep-h">
                <div class="section-head">
                    <h2 id="rc-keep-h" class="t-title-xs m-0">
                        {tr.recall.keepTitle}
                    </h2>
                </div>
                <div class="rc-rows">
                    <div class="rc-row">
                        <span class="w-text">{tr.recall.keepTalks}</span>
                        <span class="w-cap">{tr.recall.keepTalksAbout}</span>
                    </div>
                    <div class="rc-row">
                        <span class="w-text">{tr.recall.keepSpace}</span>
                        <span class="w-cap">{tr.recall.keepSpaceAbout(overview ? size(overview.limitBytes) : tr.recall.mb('8'))}</span>
                    </div>
                    <div class="rc-row">
                        <span class="w-text">{tr.recall.keepWho}</span>
                        <span class="w-cap">{tr.recall.keepWhoAbout}</span>
                    </div>
                </div>
                <p class="w-cap m-0">
                    <IconLock size={13} /> {tr.recall.voiceLock}
                </p>
            </section>
        </>
    );
}

/** Виджет стола: работает ли память и что в ней нового. По нажатию — «Приложения» → «Память». На стол его ставят сами. */
export function MemoryWidget() {
    const panel = usePanel();
    const [overview, setOverview] = useState<MemoryOverview | null>(null);
    usePoll({
        every: 60_000,
        version: panel.version,
        read: (signal) =>
            api
                .memory(signal)
                .then((fresh) => {
                    if (!signal.aborted) setOverview(fresh);
                })
                .catch(() => {}),
    });
    if (!overview) return <p class="w-cap m-0">{tr.recall.reading}</p>;
    const { state, counts, recent } = overview;
    const latest = recent.find((item) => !item.validTo);
    return (
        <div class="w-rows">
            <button class="w-row w-row-btn" onClick={() => panel.go('skills', 'memory', 'overview')}>
                <span class={`w-dot ${state.enabled && !state.paused ? 'w-dot-own' : 'w-dot-off'}`} aria-hidden="true" />
                <span class="w-text">
                    {!state.enabled ? tr.recall.widgetOff : state.paused ? (state.pausedUntil ? tr.recall.widgetPausedUntil(clock(state.pausedUntil)) : tr.recall.widgetPaused) : tr.recall.widgetOn}
                </span>
                <span class="w-cap">
                    {tr.recall.widgetRecords(counts.home)}
                </span>
            </button>
            {latest && (
                <button class="w-row w-row-btn w-row-col" onClick={() => panel.go('skills', 'memory', 'overview')}>
                    <span class="w-cap">{tr.recall.widgetNew(timeAgo(latest.createdAt))}</span>
                    <span class="w-text">{latest.text}</span>
                </button>
            )}
        </div>
    );
}

/**
 * Под ответом колонки: «Запомнено: … · Отменить» — две минуты после того, как персонаж что-то запомнил голосом или
 * мимоходом. Ошибку видно сразу, без захода в окно; добавленное в пульте человек видит и так.
 */
export function RememberedLine() {
    const panel = usePanel();
    const [change, setChange] = useState<MemoryChange | null>(null);
    const [gone, setGone] = useState<string | null>(null);
    const [now, setNow] = useState(() => Date.now());
    const allowed = can('memory.view');
    useEffect(() => {
        if (!allowed) return;
        const control = new AbortController();
        api.memoryChanges(1, control.signal)
            .then((list) => setChange(list[0] ?? null))
            .catch(() => {});
        return () => control.abort();
    }, [panel.version, allowed]);
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 15_000);
        return () => window.clearInterval(timer);
    }, []);
    if (!change || change.action !== 'add' || change.target !== 'fact' || change.undoneAt || change.actorKind === 'user') return null;
    if (gone === change.id || !change.summary || now - Date.parse(change.at) > 120_000) return null;
    const undo = () =>
        api
            .undoMemoryChange(change.id)
            .then(() => setGone(change.id))
            .catch(() => setGone(change.id));
    return (
        <div class="rc-remembered" role="status">
            <IconMemory size={14} />
            <span class="min-w-0 rc-ellipsis">{tr.recall.remembered(change.summary)}</span>
            {can('memory.manage') && (
                <button class="rc-link" onClick={() => void undo()}>
                    {tr.recall.undo}
                </button>
            )}
        </div>
    );
}
