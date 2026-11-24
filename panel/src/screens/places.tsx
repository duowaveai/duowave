// Четыре места пульта (docs/plans/panel-rebuild.md, этап 1): «Главная» — стол (desk.tsx), «Персонаж» — character.tsx,
// «Навыки» — skills.tsx, а «Настройки» — здесь. Внутри прежние экраны, они просто лежат на новых местах; старые адреса ведут сюда
// (shell/links.ts). «Настройки» — список страниц, как в настройках Mac: на широком окне список слева и страница справа,
// на телефоне сначала список, страница — по нажатию.

import { ComponentChildren, FunctionComponent } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { api, can, problemOf, type CatalogView, type HomeDevice } from '../api';
import { ask } from '../ui/confirm';
import { tr } from '../i18n';
import {
    IconChevronLeft,
    IconChevronRight,
    IconEar,
    IconFolder,
    IconGlobe,
    IconHome,
    IconJournal,
    IconKey,
    IconMic,
    IconPalette,
    IconShield,
    IconSpeaker,
    IconTerminal,
    IconVoice,
} from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { setDevMode, setHiddenDevOptions, tapDev, useDevMode, useHiddenDevOptions } from '../shell/dev';
import { Switch } from '../ui/switch';
import { useOfferedVersion } from '../updates';
import { ModeWidget, WatchWidget, useStationOffer } from './desk-widgets';
import { AwayPage } from './helpers';
import { DevicesPage } from './devices';
import { RoomCard } from './room-card';
import { Stations, Updates } from './house';
import { SetupSheet } from './stations';
import { TOP_VIA, VIA, VIA_LINK } from '../via';
import { useHomeStationName } from '../station-name';
import { JournalScreen } from './journal';
import { StationMemoryCard } from './memory';
import type { SettingsTab } from './mind';
import { HearingPage, VoicePage } from './voice-home';
import { VoiceHearingBrief } from './voice-brief';
import { useVoiceAttention } from '../voice/pending-voice';
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
            return ['settings', 'hearing'];
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
    /** Точка в списке: там что-то ждёт (новая версия, голос ждёт решения). */
    dot?: boolean;
    /** Что значит точка — для читалки экрана. */
    dotLabel?: string;
    /** Группа списка (docs/plans/menu-scheme.md): устройство, доступ и вид, система. */
    group: 'home' | 'access' | 'system';
    Page: FunctionComponent;
    /** Строка-ссылка: ведёт в свой раздел пульта, а не открывает страницу здесь. */
    go?: string;
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
 * Части страницы устройства (docs/plans/home-and-devices.md, этап 1), бывшая страница «Колонка». «Комната» колонки — её
 * имя и вид, кто отвечает и путь к Яндексу; «Устройство» — где работает DuoWave, память колонки, обновления; у
 * компьютера-центра «Этот компьютер» — как говорить с DuoWave, его микрофон и слух.
 */
export function StationRoom({ device, centre }: { device: HomeDevice; centre: boolean }) {
    const panel = usePanel();
    const [naming, setNaming] = useState(false);
    const choiceHere = useTalkChoiceHere() && can('station.view');
    const homeName = useHomeStationName();
    const station = (panel.stations ?? []).find((entry) => entry.device === device.device) ?? (centre ? panel.current : null);
    return (
        <>
            <p class="w-cap m-0">{tr.devices.roomLead}</p>
            {station && can('stations.manage') && (
                <Card id="name" title={tr.places.nameTitle}>
                    <div class="w-row">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">«{(centre ? homeName : undefined) ?? station.name}»</span>
                            <span class="w-cap">{tr.places.nameAbout}</span>
                        </span>
                        <button class="w-btn" onClick={() => setNaming(true)}>
                            {tr.places.change}
                        </button>
                    </div>
                    {naming && <SetupSheet station={station} onClose={() => setNaming(false)} onChanged={() => undefined} onLater={() => setNaming(false)} />}
                </Card>
            )}
            {centre && can('station.view') && (
                <Card id="mode" title={tr.helpers.modeRow}>
                    <ModeWidget />
                </Card>
            )}
            {device.device && can('station.view') && <RoomCard device={device.device} centre={centre} />}
            {centre && choiceHere && (
                <Card id="talk" title={tr.places.talkTitle}>
                    <TalkChoice />
                </Card>
            )}
            {centre && can('station.mode') && panel.health?.panelMode !== 'relay' && <BackToYandex />}
        </>
    );
}

export function StationDevice() {
    return (
        <>
            <p class="w-cap m-0">{tr.devices.deviceLead}</p>
            {can('station.view') && <PlacementCard />}
            {can('station.view') && <StationMemoryCard />}
            {can('station.view') && <Updates />}
        </>
    );
}

export function ComputerMic() {
    return (
        <>
            <Card id="talk" title={tr.places.talkTitle}>
                <TalkChoice />
            </Card>
            <Card id="ready" title={tr.places.readyTitle}>
                <Readiness />
            </Card>
            {/* Модели слуха и голоса — в своих пунктах «Настроек» (проверка начисто 19.11); здесь — кратко со ссылками. */}
            {can('models.manage') && <VoiceHearingBrief />}
            <Card id="mic-check" title={tr.places.micTitle}>
                <MicCheck />
            </Card>
            <Card id="talk-key" title={tr.voiceInput.keyTitle}>
                <TalkKeyChoice />
            </Card>
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
 * В окне дома на компьютере (`?via=`) — так же: сверху шлюз колонки, ниже сам компьютер (owner 20.11: колонке он ставит
 * только свою версию, и отставший компьютер должен быть виден там, где колонка просит новую).
 */
function UpdatesPage() {
    const panel = usePanel();
    const homeName = useHomeStationName();
    // Колонку обновляет этот компьютер; центр по связи (компьютер или сервер) обновляется сам.
    const turbo = panel.role.role === 'helper' && !!panel.role.station && can('station.view');
    const name = panel.role.station?.name ?? homeName;
    return (
        <>
            {turbo && (
                <Card id="copies" title={name ? tr.core.tabStation(name) : tr.core.tabStationNoName}>
                    <p class="w-cap m-0">{tr.places.stationUpdatesLead}</p>
                    <WatchWidget />
                </Card>
            )}
            <Updates title={turbo ? tr.core.tabThisComputer : VIA && !VIA_LINK && name ? tr.core.tabStation(name) : undefined} />
            {VIA && <Updates own title={tr.core.tabThisComputer} />}
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
    home: tr.devices.groupHome,
    access: tr.places.groupAccess,
    system: tr.places.groupSystem,
});

function settingsPages(offered: boolean, voiceAsks: boolean): SettingsPage[] {
    return [
        // «Дом» (docs/plans/home-and-devices.md, этап 1): устройства дома — центр, колонки и Турбо, у каждого — своя
        // страница; «Что уходит из дома» — регулятор и что будет, если Турбо уснёт. Прежние «Колонка» и «Дом»
        // (`#/settings/station`, `#/settings/helpers`) ведут сюда.
        { id: 'devices', title: tr.devices.pageTitle, sub: tr.devices.pageSub, icon: IconHome, group: 'home', Page: DevicesPage },
        // «Голос» и «Слух» — свои пункты (проверка начисто 19.11): выбор голоса и моделей не прячется в странице
        // компьютера. Точка у «Голоса» — голос ждёт решения владельца (voice/pending-voice.tsx).
        ...(can('station.view')
            ? [
                  { id: 'voice', title: tr.voiceHome.pageVoice, sub: tr.voiceHome.pageVoiceSub, icon: IconVoice, dot: voiceAsks, dotLabel: tr.voiceHome.waitDot, group: 'home' as const, Page: VoicePage },
                  { id: 'hearing', title: tr.voiceHome.pageHearing, sub: tr.voiceHome.pageHearingSub, icon: IconEar, group: 'home' as const, Page: HearingPage },
              ]
            : []),
        ...(can('connectors.view') ? [{ id: 'away', title: tr.devices.awayTitle, sub: tr.devices.awaySub, icon: IconGlobe, group: 'home' as const, Page: AwayPage }] : []),
        // «Папки» — приложение со своим окном (`#/folders`), ячейки в доке у него нет: строка «Дома» ведёт туда, иначе
        // папки было не найти на экране (находка 10 проверки шага 11).
        ...(can('station.view')
            ? [{ id: 'folders', title: tr.core.sectionFolders, sub: tr.places.pageFoldersSub, icon: IconFolder, group: 'home' as const, go: 'folders', Page: DevicesPage }]
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
    const [page, setPage] = useSectionTab<string>('settings', 'devices');
    // Телефон: сначала список; ссылка на страницу (`#/settings/updates`, поиск) открывает её сразу. Док ведёт сюда без
    // вкладки в адресе — тогда список.
    const [listed, setListed] = useState(() => !/^#\/settings\/[^/?]/.test(location.hash));
    const voiceAsks = useVoiceAttention();
    const pages = settingsPages(offered, voiceAsks);
    // Прежние «Колонка», «Дом», «Этот компьютер» и «Где работает DuoWave» — теперь «Устройства».
    const wanted = ['station', 'helpers', 'helper', 'placement'].includes(page) ? 'devices' : page;
    const shown = pages.find((entry) => entry.id === wanted && !entry.go) ?? pages[0];
    // Скрытые возможности разработчика: восемь нажатий подряд на «Для разработчиков» — строку списка или заголовок
    // открытой страницы (на телефоне список прячется после первого). Последние четыре — с подсказкой, сколько ещё.
    const [devLeft, setDevLeft] = useState<number | null>(null);
    const tapDevPage = () => {
        const left = tapDev();
        setDevLeft(left !== null && left <= 4 ? left : null);
    };
    const open = (id: string) => {
        const link = pages.find((entry) => entry.id === id)?.go;
        if (link) return panel.go(link);
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
                            {entry.dot && <span class="badge-dot" role="img" aria-label={entry.dotLabel ?? tr.places.newVersion} />}
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
