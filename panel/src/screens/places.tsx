// Четыре места пульта (docs/plans/panel-rebuild.md, этап 1): «Главная» — стол (desk.tsx), «Персонаж» — character.tsx,
// «Навыки» — skills.tsx, а «Настройки» — здесь. Внутри прежние экраны, они просто лежат на новых местах; старые адреса ведут сюда
// (shell/links.ts). «Настройки» — список страниц, как в настройках Mac: на широком окне список слева и страница справа,
// на телефоне сначала список, страница — по нажатию.

import { ComponentChildren, FunctionComponent } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { api, can, problemOf, type CatalogView } from '../api';
import { ask } from '../ui/confirm';
import { tr } from '../i18n';
import {
    IconChevronLeft,
    IconChevronRight,
    IconGlobe,
    IconHome,
    IconJournal,
    IconKey,
    IconMic,
    IconPalette,
    IconShield,
    IconSpeaker,
    IconTerminal,
} from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { setDevMode, setHiddenDevOptions, tapDev, useDevMode, useHiddenDevOptions } from '../shell/dev';
import { Switch } from '../ui/switch';
import { useOfferedVersion } from '../updates';
import { WatchWidget, useStationOffer } from './desk-widgets';
import { HelpersPage } from './helpers';
import { Stations, Updates } from './house';
import { SetupSheet } from './stations';
import { TOP_VIA } from '../via';
import { useHomeStationName } from '../station-name';
import { JournalScreen } from './journal';
import { StationMemoryCard } from './memory';
import type { SettingsTab } from './mind';
import { ModelsScreen } from './models';
import { EgressScreen } from './egress';
import { PlacementCard } from './placement';
import { SettingsScreen } from './settings';
import { MicCheck, Readiness, TalkChoice, TalkKeyChoice, useTalkChoiceHere } from './voice-input';
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

// ─── Настройки ───────────────────────────────────────────────────────────────

interface SettingsPage {
    id: string;
    title: string;
    /** Строка под именем в списке: что там. */
    sub: string;
    icon: Icon;
    /** Точка в списке: там что-то ждёт (новая версия). */
    dot?: boolean;
    /** Группа списка (docs/plans/menu-scheme.md): устройство, доступ и вид, система. */
    group: 'device' | 'access' | 'system';
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
 * «Колонка»: как говорить с DuoWave, её имя и вид, какие колонки слушаем, слух, где работает DuoWave и её память.
 * «Кто отвечает» (режим) — в «Доме» (решение владельца 16.11): там же, где Турбо и что будет без него. В режиме «Микрофон» (docs/plans/mic-mode.md) страница — «Микрофон»: тот же выбор, готовность, «Слух» и
 * проверка микрофона; о колонке на ней ничего.
 */
function StationPage() {
    const panel = usePanel();
    // Имя и вид — тот же лист, что при первой настройке: прежде его нельзя было открыть снова.
    const [naming, setNaming] = useState(false);
    const current = panel.current;
    const computer = useOnComputer();
    const choiceHere = useTalkChoiceHere() && can('station.view');
    // Одно имя колонки (владелец 16.11): в окне колонки на компьютере — имя из его списка.
    const homeName = useHomeStationName();
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
                <Card id="talk-key" title={tr.voiceInput.keyTitle}>
                    <TalkKeyChoice />
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
                            <span class="w-strong">«{homeName ?? current.name}»</span>
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

function AccessPage() {
    const panel = usePanel();
    return <SettingsScreen catalog={panel.catalog} tab="access" onTab={() => undefined} onSignOut={panel.signOut} bare />;
}

/**
 * «Оформление» — вкладка «Оформление» прежних «Настроек» целиком: язык, вид пульта, стол (лента ярлыков) и «Пройти
 * первые шаги». Со списка страниц (этап 1 переделки) здесь был только вид пульта — остальное не открывалось ниоткуда.
 */
function LookPage() {
    const panel = usePanel();
    return <SettingsScreen catalog={panel.catalog} tab="look" onTab={() => undefined} onSignOut={panel.signOut} bare />;
}

/**
 * «Обновления и копии». В пульте Турбо колонка — первой (owner 17.11): её шлюз обновляет этот компьютер, и новая версия
 * для неё — именно здесь; ниже — версия самого компьютера, под своим именем, чтобы «Новых версий нет» не спорило с ней.
 */
function UpdatesPage() {
    const panel = usePanel();
    const homeName = useHomeStationName();
    const turbo = panel.role.role === 'helper' && can('station.view');
    const name = panel.role.station?.name ?? homeName;
    return (
        <>
            {turbo && (
                <Card id="copies" title={name ? tr.core.tabStation(name) : tr.core.tabStationNoName}>
                    <p class="w-cap m-0">{tr.places.stationUpdatesLead}</p>
                    <WatchWidget />
                </Card>
            )}
            <Updates title={turbo ? tr.core.tabThisComputer : undefined} />
        </>
    );
}

function HistoryPage() {
    const panel = usePanel();
    return <JournalScreen debug={panel.debug} catalog={panel.catalog} live={panel.feed.requests} stations={panel.stations} />;
}

/**
 * «Для разработчиков»: переключатель третьего слоя. Он не добавляет ни кнопок в док, ни списка ярлыков: технические
 * строки появляются на своих местах (docs/plans/menu-scheme.md) — таблица «Кто отвечает» в «Моих», ключи API в «Людях
 * и доступе», модели в «Мозге», места разработчика в поиске ⌘K.
 */
function DevPage() {
    const dev = useDevMode();
    const hidden = useHiddenDevOptions();
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
            {hidden && <HiddenDevOptions />}
        </>
    );
}

/**
 * Скрытые возможности разработчика (восемь нажатий на «Для разработчиков»). «Показывать скрытые пакеты» — настройка
 * шлюза packages.show_hidden: пакеты, которые ленты держат скрытыми (`unlisted`), видны в MagicPlace, в каталоге
 * персонажей и голосом — из главного каталога, магазинов и своих источников. Меняет администратор.
 */
function HiddenDevOptions() {
    const [view, setView] = useState<CatalogView | null>(null);
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    useEffect(() => {
        let alive = true;
        api.packagesCatalog()
            .then((next) => alive && setView(next))
            .catch((error) => alive && setProblem(problemOf(error)));
        return () => {
            alive = false;
        };
    }, []);
    const toggle = async (on: boolean) => {
        setBusy(true);
        setProblem(null);
        try {
            setView(await api.setShowHiddenPackages(on));
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(false);
        }
    };
    const shown = view?.packages.filter((entry) => entry.unlisted) ?? [];
    return (
        <Card id="dev-hidden" title={tr.places.hiddenTitle}>
            <label class="w-toggle">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{tr.places.showHiddenPackages}</span>
                    <span class="w-cap">{tr.places.showHiddenPackagesAbout}</span>
                </span>
                <Switch
                    on={!!view?.showHidden}
                    label={tr.places.showHiddenPackages}
                    disabled={!view || busy || !can('settings.edit')}
                    onChange={(on) => void toggle(on)}
                />
            </label>
            {view?.showHidden && (
                <p class="w-cap">{shown.length ? tr.places.hiddenPackagesNow(shown.map((entry) => entry.title).join(', ')) : tr.places.hiddenPackagesNone}</p>
            )}
            {problem && (
                <p class="w-cap" role="alert">
                    {problem}
                </p>
            )}
            <button class="w-btn" onClick={() => setHiddenDevOptions(false)}>
                {tr.places.hiddenClose}
            </button>
        </Card>
    );
}

const GROUP_TITLES = (): Record<SettingsPage['group'], string> => ({
    device: tr.places.groupDevice,
    access: tr.places.groupAccess,
    system: tr.places.groupSystem,
});

function settingsPages(role: string, offered: boolean): SettingsPage[] {
    return [
        // Адрес страницы прежний (`#/settings/station`); в режиме «Микрофон» она — о микрофоне.
        onComputer()
            ? { id: 'station', title: tr.places.pageMic, sub: tr.places.pageMicSub, icon: IconMic, group: 'device' as const, Page: StationPage }
            : { id: 'station', title: tr.places.pageStation, sub: tr.places.pageStationSub, icon: IconSpeaker, group: 'device' as const, Page: StationPage },
        // «Дом» (решение владельца 16.11) — одна страница вместо «Помощников» и «Этого компьютера»: у шлюза — он, его
        // Турбо и что будет без них; у пульта Турбо — его силы и кому он помогает. Адрес прежний (`#/settings/helpers`).
        ...(can('connectors.view') || TOP_VIA || role === 'helper'
            ? [
                  {
                      id: 'helpers',
                      title: tr.helpers.pageTitle,
                      sub: role === 'helper' && !TOP_VIA ? tr.helpers.pageSubTurbo : tr.helpers.pageSub,
                      icon: IconHome,
                      group: 'device' as const,
                      Page: HelpersPage,
                  },
              ]
            : []),
        { id: 'access', title: tr.places.pageAccess, sub: tr.places.pageAccessSub, icon: IconKey, group: 'access', Page: AccessPage },
        { id: 'look', title: tr.places.pageLook, sub: tr.places.pageLookSub, icon: IconPalette, group: 'access', Page: LookPage },
        ...(can('station.view')
            ? [{ id: 'updates', title: tr.places.pageUpdates, sub: tr.places.pageUpdatesSub, icon: IconShield, dot: offered, group: 'system' as const, Page: UpdatesPage }]
            : []),
        // «Выход в интернет» (docs/plans/own-server.md): через какой свой сервер что идёт; туда же ведёт окно «VPN».
        ...(can('connectors.view') ? [{ id: 'egress', title: tr.egress.title, sub: tr.egress.sub, icon: IconGlobe, group: 'system' as const, Page: EgressScreen }] : []),
        ...(can('journal.view') ? [{ id: 'history', title: tr.places.pageHistory, sub: tr.places.pageHistorySub, icon: IconJournal, group: 'system' as const, Page: HistoryPage }] : []),
        { id: 'dev', title: tr.places.devTitle, sub: tr.places.pageDevSub, icon: IconTerminal, group: 'system', Page: DevPage },
    ];
}

/** «Настройки»: список страниц и открытая страница. */
export function SettingsSection() {
    const panel = usePanel();
    const stationOffer = useStationOffer(panel.role.role === 'helper');
    const offered = !!useOfferedVersion() || !!stationOffer;
    // Список перерисовывается, когда меняется «Как говорить»: страница колонки становится страницей микрофона.
    useOnComputer();
    const [page, setPage] = useSectionTab<string>('settings', 'station');
    // Телефон: сначала список; ссылка на страницу (`#/settings/updates`, поиск) открывает её сразу. Док ведёт сюда без
    // вкладки в адресе — тогда список.
    const [listed, setListed] = useState(() => !/^#\/settings\/[^/?]/.test(location.hash));
    const pages = settingsPages(panel.role.role, offered);
    // Прежние «Этот компьютер» и «Где работает DuoWave» (`#/settings/helper`, `#/settings/placement`) — теперь «Дом».
    const wanted = page === 'helper' || page === 'placement' ? 'helpers' : page;
    const shown = pages.find((entry) => entry.id === wanted) ?? pages[0];
    // Скрытые возможности разработчика: восемь нажатий подряд на «Для разработчиков» — строку списка или заголовок
    // открытой страницы (на телефоне список прячется после первого). Последние четыре — с подсказкой, сколько ещё.
    const [devLeft, setDevLeft] = useState<number | null>(null);
    const tapDevPage = () => {
        const left = tapDev();
        setDevLeft(left !== null && left <= 4 ? left : null);
    };
    const open = (id: string) => {
        if (id === 'dev') tapDevPage();
        setListed(false);
        setPage(id);
    };
    const devHint = devLeft === null ? null : devLeft === 0 ? tr.places.hiddenOpened : tr.places.hiddenTapsLeft(devLeft);
    const { Page } = shown;
    return (
        <div class="set-split" data-view={listed ? 'list' : 'page'}>
            <nav class="set-list" aria-label={tr.places.settings}>
                {pages.map((entry, at) => {
                    const { icon: PageIcon } = entry;
                    // Подпись группы — над её первой страницей: «Устройство», «Доступ и вид», «Система».
                    const head = at === 0 || pages[at - 1].group !== entry.group ? GROUP_TITLES()[entry.group] : null;
                    return [
                        head && (
                            <span key={`group-${entry.group}`} class="set-group">
                                {head}
                            </span>
                        ),
                        <button key={entry.id} class="set-row" aria-current={entry.id === shown.id ? 'page' : undefined} onClick={() => open(entry.id)}>
                            <span class="set-row-icon" aria-hidden="true">
                                <PageIcon size={18} />
                            </span>
                            <span class="set-row-text">
                                <b>{entry.title}</b>
                                <span class="w-cap">{entry.id === 'dev' && devHint ? devHint : entry.sub}</span>
                            </span>
                            {entry.dot && <span class="badge-dot" role="img" aria-label={tr.places.newVersion} />}
                            <IconChevronRight size={16} />
                        </button>,
                    ];
                })}
            </nav>
            <div class="set-page" aria-label={shown.title}>
                <button class="set-back" onClick={() => setListed(true)}>
                    <IconChevronLeft size={16} />
                    {tr.places.settings}
                </button>
                {shown.id === 'dev' ? (
                    <h2 class="set-page-title" onClick={tapDevPage}>
                        {shown.title}
                    </h2>
                ) : (
                    <h2 class="set-page-title">{shown.title}</h2>
                )}
                {shown.id === 'dev' && devHint && (
                    <p class="w-cap" role="status">
                        {devHint}
                    </p>
                )}
                <Page key={shown.id} />
            </div>
        </div>
    );
}
