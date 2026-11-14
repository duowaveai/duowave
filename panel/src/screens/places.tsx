// Четыре места пульта (docs/plans/panel-rebuild.md, этап 1): «Главная» — стол (desk.tsx), «Персонаж» — character.tsx,
// «Навыки» — skills.tsx, а «Настройки» — здесь. Внутри прежние экраны, они просто лежат на новых местах; старые адреса ведут сюда
// (shell/links.ts). «Настройки» — список страниц, как в настройках Mac: на широком окне список слева и страница справа,
// на телефоне сначала список, страница — по нажатию.

import { ComponentChildren, FunctionComponent } from 'preact';
import { useState } from 'preact/hooks';

import { api, can, problemOf, sessionStore } from '../api';
import { ask } from '../ui/confirm';
import { tr } from '../i18n';
import {
    IconChevronLeft,
    IconChevronRight,
    IconJournal,
    IconKey,
    IconLaptop,
    IconMic,
    IconPalette,
    IconShield,
    IconSpeaker,
    IconTerminal,
} from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { setDevMode, useDevMode } from '../shell/dev';
import { ThemeCard } from '../shell/theme-picker';
import { Switch } from '../ui/switch';
import { useOfferedVersion } from '../updates';
import { HelperCardWidget, HelperWidget, ModeWidget, WatchWidget } from './desk-widgets';
import { Stations, Updates } from './house';
import { SetupSheet } from './stations';
import { OwnFrame } from './helper-pc';
import { TOP_VIA } from '../via';
import { JournalScreen } from './journal';
import { StationMemoryCard } from './memory';
import type { SettingsTab } from './mind';
import { ModelsScreen } from './models';
import { PlacementCard } from './placement';
import { SettingsScreen } from './settings';
import { MicCheck, Readiness, TalkChoice, useTalkChoiceHere } from './voice-input';
import { onComputer, useOnComputer } from '../voice-input';

type Icon = FunctionComponent<{ size?: number }>;

/** Ссылки дашборда и экранов «в Мозг», «в Голос», «в Распознавание» — на их новые места. */
export const settingsPlace = (place: SettingsTab): [string, string] => {
    switch (place) {
        case 'routes':
            return ['skills', 'routes'];
        case 'recognition':
            return ['settings', 'station'];
        case 'character':
            return ['character', 'page'];
        default:
            return ['character', place];
    }
};

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

// ─── Настройки ───────────────────────────────────────────────────────────────

interface SettingsPage {
    id: string;
    title: string;
    /** Строка под именем в списке: что там. */
    sub: string;
    icon: Icon;
    /** Точка в списке: там что-то ждёт (новая версия). */
    dot?: boolean;
    Page: FunctionComponent;
}

/** Карточка страницы «Настроек» с заголовком — для того, у чего своего заголовка нет (виджеты). */
function Card({ id, title, children }: { id: string; title: string; children: ComponentChildren }) {
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`set-${id}`}>
            <div class="section-head">
                <h2 id={`set-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
            </div>
            {children}
        </section>
    );
}

/**
 * «Колонка»: как говорить с DuoWave, её имя и вид, какие колонки слушаем, режим, слух, где работает DuoWave и её
 * память. В режиме «Микрофон» (docs/plans/mic-mode.md) страница — «Микрофон»: тот же выбор, готовность, «Слух» и
 * проверка микрофона; о колонке на ней ничего.
 */
function StationPage() {
    const panel = usePanel();
    // Имя и вид — тот же лист, что при первой настройке: прежде его нельзя было открыть снова.
    const [naming, setNaming] = useState(false);
    const current = panel.current;
    const computer = useOnComputer();
    const choiceHere = useTalkChoiceHere() && can('station.view');
    if (computer)
        return (
            <>
                <Card id="talk" title={tr.places.talkTitle}>
                    <TalkChoice />
                </Card>
                <Card id="ready" title={tr.places.readyTitle}>
                    <Readiness />
                </Card>
                <ModelsScreen tab="recognition" onTab={() => undefined} bare />
                <Card id="mic-check" title={tr.places.micTitle}>
                    <MicCheck />
                </Card>
            </>
        );
    return (
        <>
            {choiceHere && (
                <Card id="talk" title={tr.places.talkTitle}>
                    <TalkChoice />
                </Card>
            )}
            {current && can('stations.manage') && (
                <Card id="name" title={tr.places.nameTitle}>
                    <div class="w-row">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">«{current.name}»</span>
                            <span class="w-cap">{tr.places.nameAbout}</span>
                        </span>
                        <button class="w-btn" onClick={() => setNaming(true)}>
                            {tr.places.change}
                        </button>
                    </div>
                    {naming && <SetupSheet station={current} onClose={() => setNaming(false)} onChanged={() => undefined} onLater={() => setNaming(false)} />}
                </Card>
            )}
            {can('station.view') && <Stations />}
            {panel.role.role !== 'helper' && can('station.view') && (
                <Card id="mode" title={tr.places.modeTitle}>
                    <ModeWidget />
                </Card>
            )}
            <ModelsScreen tab="recognition" onTab={() => undefined} bare />
            {can('station.view') && <PlacementCard />}
            {can('station.view') && <StationMemoryCard />}
            {can('station.mode') && panel.health?.panelMode !== 'relay' && <BackToYandex />}
        </>
    );
}

/**
 * Один путь к Яндексу (docs/plans/panel-rebuild.md, этап 6) вместо пяти дверей с разными табличками: колонку целиком
 * ведёт Яндекс, как до DuoWave. Обратимо — режимом выше; DuoWave на колонке остаётся и ждёт.
 */
function BackToYandex() {
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const back = async () => {
        if (
            !(await ask({
                title: tr.places.backAskTitle,
                text: tr.places.backAskText,
                ok: tr.places.backAskOk,
            }))
        )
            return;
        setBusy(true);
        setNote(null);
        try {
            await api.setMode('relay');
            setNote({ ok: true, text: tr.places.backDone });
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    return (
        <Card id="yandex" title={tr.places.backTitle}>
            <p class="w-cap m-0">{tr.places.backAbout}</p>
            <div>
                <button class="w-btn" disabled={busy} onClick={() => void back()}>
                    {busy ? tr.places.backBusy : tr.places.backTitle}
                </button>
            </div>
            {note && (
                <p role={note.ok ? 'status' : 'alert'} class={`w-cap m-0 ${note.ok ? 'c-text' : 'c-dot-offline'}`}>
                    {note.text}
                </p>
            )}
        </Card>
    );
}

/** «Компьютер-помощник»: что компьютер даёт колонке и как присматривает за ней. Одной страницей — этап 4. */
function HelperPage() {
    const panel = usePanel();
    // Окно приложения — пульт колонки (этап 4): своё у компьютера — его страница, напрямую, без прохода.
    if (TOP_VIA) return <OwnFrame page="helper" title={tr.places.helperTitle} />;
    if (panel.role.role === 'helper')
        return (
            <>
                <Card id="helper" title={tr.places.helpTitle}>
                    <HelperWidget />
                </Card>
                <Card id="watch" title={tr.places.watchTitle}>
                    <WatchWidget />
                </Card>
            </>
        );
    return (
        <Card id="helper-card" title={tr.places.helperTitle}>
            <HelperCardWidget />
        </Card>
    );
}

function AccessPage() {
    const panel = usePanel();
    return <SettingsScreen catalog={panel.catalog} tab="access" onTab={() => undefined} onSignOut={panel.signOut} bare />;
}

function UpdatesPage() {
    const panel = usePanel();
    return (
        <>
            <Updates />
            {panel.role.role === 'helper' && can('station.view') && (
                <Card id="copies" title={tr.places.copiesTitle}>
                    <WatchWidget />
                </Card>
            )}
        </>
    );
}

function HistoryPage() {
    const panel = usePanel();
    return <JournalScreen debug={panel.debug} catalog={panel.catalog} live={panel.feed.requests} stations={panel.stations} />;
}

/** «Для разработчиков»: переключатель третьего слоя и куда он ведёт. */
function DevPage() {
    const panel = usePanel();
    const dev = useDevMode();
    const places: Array<{ title: string; sub: string; go: () => void; shown: boolean }> = [
        { title: tr.places.workshop, sub: tr.places.workshopSub, go: () => panel.go('skills', 'workshop'), shown: isAdmin() },
        { title: tr.places.routesTitle, sub: tr.places.routesSub, go: () => panel.go('skills', 'routes'), shown: !onComputer() },
        { title: tr.places.modelsTitle, sub: tr.places.modelsSub, go: () => panel.go('character', 'brain'), shown: true },
        { title: tr.places.mcpTitle, sub: tr.places.mcpSub, go: () => panel.go('skills', 'connectors'), shown: can('connectors.view') },
        { title: tr.places.journalTitle, sub: tr.places.journalSub, go: () => panel.go('settings', 'history'), shown: can('journal.view') },
        { title: tr.places.keysTitle, sub: tr.places.keysSub, go: () => panel.go('settings', 'access'), shown: can('users.manage') },
    ];
    return (
        <>
            <Card id="dev" title={tr.places.devTitle}>
                <label class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{tr.places.showDev}</span>
                        <span class="w-cap">{tr.places.showDevAbout}</span>
                    </span>
                    <Switch on={dev} label={tr.places.devLayer} onChange={setDevMode} />
                </label>
            </Card>
            {dev && (
                <section class="card glass card-list" aria-label={tr.places.devPlaces}>
                    <div class="w-rows">
                        {places
                            .filter((place) => place.shown)
                            .map((place) => (
                                <button key={place.title} class="w-row w-row-btn" onClick={place.go}>
                                    <span class="flex-1 min-w-0 flex flex-col">
                                        <span class="w-text">{place.title}</span>
                                        <span class="w-cap">{place.sub}</span>
                                    </span>
                                    <IconChevronRight size={16} />
                                </button>
                            ))}
                    </div>
                </section>
            )}
        </>
    );
}

function settingsPages(role: string, onStation: boolean, offered: boolean): SettingsPage[] {
    return [
        // Адрес страницы прежний (`#/settings/station`); в режиме «Микрофон» она — о микрофоне.
        onComputer()
            ? { id: 'station', title: tr.places.pageMic, sub: tr.places.pageMicSub, icon: IconMic, Page: StationPage }
            : { id: 'station', title: tr.places.pageStation, sub: tr.places.pageStationSub, icon: IconSpeaker, Page: StationPage },
        ...(role === 'helper' || onStation
            ? [{ id: 'helper', title: tr.places.helperTitle, sub: tr.places.pageHelperSub, icon: IconLaptop, Page: HelperPage }]
            : []),
        { id: 'access', title: tr.places.pageAccess, sub: tr.places.pageAccessSub, icon: IconKey, Page: AccessPage },
        { id: 'look', title: tr.places.pageLook, sub: tr.places.pageLookSub, icon: IconPalette, Page: ThemeCard },
        ...(can('station.view')
            ? [{ id: 'updates', title: tr.places.pageUpdates, sub: tr.places.pageUpdatesSub, icon: IconShield, dot: offered, Page: UpdatesPage }]
            : []),
        ...(can('journal.view') ? [{ id: 'history', title: tr.places.pageHistory, sub: tr.places.pageHistorySub, icon: IconJournal, Page: HistoryPage }] : []),
        { id: 'dev', title: tr.places.devTitle, sub: tr.places.pageDevSub, icon: IconTerminal, Page: DevPage },
    ];
}

/** «Настройки»: список страниц и открытая страница. */
export function SettingsSection() {
    const panel = usePanel();
    const offered = !!useOfferedVersion();
    // Список перерисовывается, когда меняется «Как говорить»: страница колонки становится страницей микрофона.
    useOnComputer();
    const [page, setPage] = useSectionTab<string>('settings', 'station');
    // Телефон: сначала список; ссылка на страницу (`#/settings/updates`, поиск) открывает её сразу. Док ведёт сюда без
    // вкладки в адресе — тогда список.
    const [listed, setListed] = useState(() => !/^#\/settings\/[^/?]/.test(location.hash));
    const onStation = panel.health?.placement?.role === 'station';
    const pages = settingsPages(panel.role.role, onStation, offered);
    const shown = pages.find((entry) => entry.id === page) ?? pages[0];
    const open = (id: string) => {
        setListed(false);
        setPage(id);
    };
    const { Page } = shown;
    return (
        <div class="set-split" data-view={listed ? 'list' : 'page'}>
            <nav class="set-list" aria-label={tr.places.settings}>
                {pages.map((entry) => {
                    const { icon: PageIcon } = entry;
                    return (
                        <button key={entry.id} class="set-row" aria-current={entry.id === shown.id ? 'page' : undefined} onClick={() => open(entry.id)}>
                            <span class="set-row-icon" aria-hidden="true">
                                <PageIcon size={18} />
                            </span>
                            <span class="set-row-text">
                                <b>{entry.title}</b>
                                <span class="w-cap">{entry.sub}</span>
                            </span>
                            {entry.dot && <span class="badge-dot" role="img" aria-label={tr.places.newVersion} />}
                            <IconChevronRight size={16} />
                        </button>
                    );
                })}
            </nav>
            <div class="set-page" aria-label={shown.title}>
                <button class="set-back" onClick={() => setListed(true)}>
                    <IconChevronLeft size={16} />
                    {tr.places.settings}
                </button>
                <h2 class="set-page-title">{shown.title}</h2>
                <Page key={shown.id} />
            </div>
        </div>
    );
}
