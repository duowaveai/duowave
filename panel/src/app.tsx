import { useEffect, useRef, useState } from 'preact/hooks';

import { AccessCatalog, ApiError, Health, MovedGateway, SessionSnapshot, Station, UnsupportedEntry, api, can, sessionStore } from './api';
import { noteAvailability } from './availability';
import { settingsPlace } from './builtin';
import { IconChevronDown, IconCode, IconSearch } from './icons';
import { useLive, useStationLive } from './live';
import { Reach, dotClass, isReachable, reachOf } from './presence';
import { AuthScreen } from './screens/auth';
import { SaySheet } from './screens/say';
import { ConnectSheet } from './screens/connect';
import { SetupBanner, SetupSheet, StationButton, StationsPicker, displayName, needsSetup } from './screens/stations';
import { DebugState, PanelContext, PanelState } from './shell/context';
import { Place, hashOf, normalizePlace, parsePlace, writePlace } from './shell/links';
import { AccountMenu } from './shell/account-menu';
import { useDevMode } from './shell/dev';
import { SearchPalette, SearchPlace, searchPlaces, useSearchKey } from './shell/search';
import { timeAgo } from './time';
import { SectionDef, useSections } from './shell/sections';
import { Nav, NavEntry, OpenWindow, TopLine } from './shell/shell';
import { applyRole, ownName, roleOf, RoleInfo } from './shell/role';
import { shellFor } from './shell/shells';
import { Boundary, Slot } from './shell/slot';
import { useTheme } from './shell/theme';
import { drawingOf } from './stations';
import { MovedBanner } from './ui/moved-banner';
import { UpdateBanner } from './ui/update-banner';
import { readUpdateStatus, useOfferedUpdate } from './updates';
import { syncPackages } from './plugins/loader';
import { notePackageChange, runtimeOnly } from './plugins/versions';
import { forgetStationPackages } from './sdk';
import { AddWidgetPanel } from './screens/add-widget';
import { WidgetAgent } from './screens/widget-agent';
import { deskActions } from './shell/widgets';
import { StationFrame, StationModeChip, ViaRefused, WindowShell } from './shell/station-window';
import { leaving, mayLeave } from './shell/leave';
import { LOCAL, OWN_PAGE, TOP_VIA, VIA, WINDOW } from './via';
import { OwnPage } from './screens/helper-pc';
import { ViaNotice } from './shell/station-window';

/**
 * Пульт колонки во всё окно (этап 4) смотрит, помогает ли ей ещё этот компьютер: его /health — напрямую, без прохода.
 * Перестал (шлюз вернулся на компьютер, помощь выключили) — окно снова пульт компьютера.
 */
function useLeaveWhenNotHelping(): void {
    useEffect(() => {
        if (!TOP_VIA || !VIA) return;
        const check = async (): Promise<void> => {
            try {
                const response = await fetch('/health', { headers: { Accept: 'application/json' } });
                if (!response.ok) return;
                const health = ((await response.json()) as { message?: Health }).message;
                const placement = health?.placement;
                if (!placement) return;
                const placed = placement.stations?.find((entry) => entry.key === VIA) ?? (placement.station?.key === VIA ? placement.station : undefined);
                if (placement.role === 'pc' && placed && placed.placement !== 'pc') return;
                location.replace(`${location.pathname}${location.hash}`);
            } catch {
                // компьютер не ответил — ничего не решаем
            }
        };
        const id = window.setInterval(() => void check(), 30_000);
        return () => window.clearInterval(id);
    }, []);
}

/** Живая колонка или последняя закрытая сессия: когда списка колонок нет (нет права, шлюз постарше). */
function pickStation(debug: DebugState): SessionSnapshot | undefined {
    const live = debug.sessions?.find((s) => !s.closedAt);
    return live ?? debug.sessions?.[0];
}

/** Что пульт помнит в этом браузере: выбранную колонку и скрытые плашки о настройке. */
function remembered(key: string): string | null {
    try {
        return localStorage.getItem(key);
    } catch {
        return null;
    }
}

function remember(key: string, value: string): void {
    try {
        localStorage.setItem(key, value);
    } catch {
        // приватное окно: помним до перезагрузки
    }
}

/** Как колонку слышно — для строки состояния: «на связи», «не на связи · была 5 мин назад». */
function reachWords(reach: Reach, lastSeen?: string): string {
    switch (reach) {
        case 'online':
            return 'на связи';
        case 'waiting':
            return 'на связи · ждёт «Алиса»';
        case 'reconnecting':
            return 'переподключается…';
        case 'unknown':
            return 'проверяю связь…';
        default:
            return lastSeen ? `не на связи · была ${timeAgo(lastSeen)}` : 'не на связи';
    }
}

/**
 * Компьютер рядом с колонкой — второй половиной строки: «компьютер помогает». Одно имя вместо «помощника», «ПК» и
 * «Колонки + ПК» (docs/plans/panel-rebuild.md, «Словарь»). Шлюз на компьютере без колонки со своим шлюзом — ничего.
 */
function helpWords(role: RoleInfo): string | null {
    if (role.role === 'helper') return role.helping ? 'компьютер помогает' : 'колонка работает сама';
    const helper = role.helper;
    if (!helper || !helper.enabled) return null;
    return helper.state === 'up' ? 'компьютер помогает' : 'компьютер не на связи';
}

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/**
 * Док (docs/plans/panel-rebuild.md): «Главная · Персонаж · [Сказать] · Навыки · Настройки» — у всех ролей; разделы
 * плагинов — перед «Настройками»; «Мастерская» — у разработчика. Помощнику своего дока не нужно: его окно — пульт
 * колонки (этап 4).
 */
function navOf(sections: SectionDef[], role: RoleInfo, dev: boolean): Nav {
    const byId = new Map(sections.map((section) => [section.id, section]));
    const entry = (id: string, over: Partial<NavEntry> = {}): NavEntry[] => {
        const section = byId.get(id);
        return section ? [{ key: id, title: section.title, icon: section.icon, Badge: section.Badge, section: id, ...over }] : [];
    };
    const core = new Set(['home', 'character', 'skills', 'settings']);
    const extra = sections.filter((section) => !core.has(section.id)).flatMap((section) => entry(section.id));
    const workshop = dev && isAdmin() ? entry('skills', { key: 'skills/workshop', title: 'Мастерская', tab: 'workshop', icon: IconCode }) : [];
    return {
        left: [...entry('home'), ...entry('character')],
        right: [...entry('skills'), ...workshop, ...extra, ...entry('settings', { tab: '' })],
    };
}

/**
 * Память вкладок: свой раздел — по id, раздел колонки у помощника — отдельно (`station:<id>`): вкладка своих
 * «Плагинов» не открывается в окне колонки, и наоборот.
 */
const tabKey = (section: string, query: Record<string, string>): string => (query.on === 'station' ? `station:${section}` : section);

/** Какой пункт навигации открыт: стол, раздел колонки у помощника, мастерская или раздел. */
function activeKey(place: Place, role: RoleInfo): string {
    if (place.section === 'home') return 'home';
    if (place.section === 'skills' && place.tab === 'workshop' && place.query.on !== 'station') return 'skills/workshop';
    if (role.role === 'helper' && place.query.on === 'station') return `station:${place.section}`;
    return place.section;
}

export function App() {
    const [auth, setAuth] = useState<'loading' | 'setup' | 'login' | 'ok'>('loading');
    const [accounts, setAccounts] = useState(1);
    const [needsCode, setNeedsCode] = useState(false);
    const [health, setHealth] = useState<Health | null>(null);
    const [offline, setOffline] = useState(false);
    const [debug, setDebug] = useState<DebugState>({ sessions: null, unsupported: [], error: null });
    const [catalog, setCatalog] = useState<AccessCatalog | null>(null);
    /** Куда вела ссылка, с которой открыли пульт (`#/<раздел>/<вкладка>/<id>`): ждёт входа и открывается после него. */
    const [boot] = useState(() => parsePlace(location.hash));
    const [section, setSection] = useState(() => boot?.section ?? WINDOW ?? 'home');
    /** Вкладки внутри разделов: раздел помнит, где его оставили, а шиты дашборда ведут прямо к нужной. */
    const [tabs, setTabs] = useState<Record<string, string>>(() => (boot?.tab ? { [tabKey(boot.section, boot.query)]: boot.tab } : {}));
    /** Что открыто во вкладке по адресу (лист подключения, карточка персонажа) и параметры ссылки. */
    const [opened, setOpened] = useState<{ item?: string; query: Record<string, string> }>(() => ({
        item: boot?.tab ? boot.item : undefined,
        query: boot?.query ?? {},
    }));
    const [sayOpen, setSayOpen] = useState(false);
    /** «+» — новый виджет на стол. */
    const [addOpen, setAddOpen] = useState(false);
    /** Колонки дома из записей шлюза; null — список не прочитать (нет права или шлюз постарше). */
    const [stations, setStations] = useState<Station[] | null>(null);
    const [currentId, setCurrentId] = useState<string | null>(() => remembered('glass-station'));
    const [pickerOpen, setPickerOpen] = useState(false);
    /** Мастер «Подключить колонку» с главного экрана и сразу после первой настройки (из меню колонок — свой). */
    const [connectOpen, setConnectOpen] = useState(false);
    /** Колонки, которые ждут шлюз на прежнем порту этого компьютера, и та, что переподключают по «Переподключить». */
    const [moved, setMoved] = useState<MovedGateway[]>([]);
    const [reconnect, setReconnect] = useState<MovedGateway | null>(null);
    const [setupId, setSetupId] = useState<string | null>(null);
    const [dismissed, setDismissed] = useState<string[]>(() => (remembered('glass-setup-later') ?? '').split(',').filter(Boolean));
    /** Растёт с каждой переменой, о которой сказал шлюз: дашборд перечитывает своё. */
    const [version, setVersion] = useState(0);
    /** Растёт, когда меняются пакеты: пульт заново берёт темы пакетов и части плагинов (plugins/loader.ts). */
    const [packagesVersion, setPackagesVersion] = useState(0);
    const pollNow = useRef<() => void>();
    const theme = useTheme();
    /** Слой разработчика (shell/dev.ts): «Мастерская» в доке, технические места в поиске. */
    const dev = useDevMode();
    /** Поиск по пульту: ⌘K или Ctrl+K, лупа в верхней строке. */
    const [searchOpen, setSearchOpen] = useState(false);
    useSearchKey(() => setSearchOpen(true));
    // Живой канал: присутствие колонок и запросы на ходу; перемена где-то ещё — перечитать сейчас, не ждать опроса.
    const feed = useLive(auth === 'ok' && can('station.view'), (name, subject, change) => {
        // Новая версия или канал открылся заново (пока его не было, шлюз мог найти версию или поставить её).
        if (name === 'update.available' || name === 'hello') void readUpdateStatus();
        // Части плагинов меняются, когда пакеты включают, ставят, обновляют, — не когда их код загрузился или выгрузился.
        if ((name === 'packages.changed' && !runtimeOnly(change)) || name === 'hello') setPackagesVersion((value) => value + 1);
        // Виджеты плагинов перечитывают своё только по переменам своего пакета (plugins/versions.ts).
        notePackageChange(name, subject, change);
        setVersion((value) => value + 1);
        readAvailability.current = 0;
        pollNow.current?.();
    });
    // Темы пакетов и части плагинов с шлюза — после входа и при каждой перемене пакетов (не в безопасном режиме ?safe=1).
    useEffect(() => {
        if (auth === 'ok' && can('station.view')) void syncPackages();
    }, [auth, packagesVersion]);
    /** Когда пульт последний раз читал, что где доступно: перечитывает при каждой перемене на шлюзе и раз в минуту. */
    const readAvailability = useRef(0);

    const readStations = (): Promise<void> =>
        can('station.view')
            ? api
                  .stations()
                  .then(setStations)
                  .catch(() => {})
            : Promise.resolve();

    /** Кто вошёл и что ему можно: права приходят от шлюза, пульт их только показывает. */
    const learnWhoIAm = async (): Promise<boolean> => {
        try {
            const me = await api.me();
            const session = sessionStore.get();
            if (session) sessionStore.set({ ...session, name: me.name, role: me.role, permissions: me.permissions });
            void api
                .catalog()
                .then(setCatalog)
                .catch(() => {});
            return true;
        } catch (e) {
            if (e instanceof ApiError && e.status === 401) {
                sessionStore.clear();
                return false;
            }
            return true; // шлюз молчит — панель покажет баннер недоступности
        }
    };

    // Первый вопрос пульта — состоянию шлюза: показать визард, вход или сразу панель.
    useEffect(() => {
        void (async () => {
            try {
                const state = await api.panelState();
                setAccounts(state.accounts);
                setNeedsCode(state.needsCode);
                if (!state.configured) return setAuth('setup');
            } catch {
                return setAuth('ok'); // шлюз молчит — панель покажет баннер недоступности
            }
            if (!sessionStore.get()) return setAuth('login');
            setAuth((await learnWhoIAm()) ? 'ok' : 'login');
        })();
    }, []);

    // Что шлюз знает о новой версии: для точки на «Дом» и строки на главном экране. Без живого канала — раз в час.
    useEffect(() => {
        if (auth !== 'ok') return;
        void readUpdateStatus();
        const timer = window.setInterval(() => void readUpdateStatus(), 60 * 60_000);
        return () => window.clearInterval(timer);
    }, [auth]);
    const offered = useOfferedUpdate();
    const [hiddenUpdate, setHiddenUpdate] = useState(() => remembered('glass-update-later'));

    // Шлюз сменил порт (другой был занят), а колонка ходит на прежний: её голос никуда не доходит. Читается при входе
    // и после мастера подключения; без права подключать колонки — ничего.
    const readMoved = (): void => {
        if (!can('stations.manage')) return;
        api.movedGateway()
            .then(setMoved)
            .catch(() => setMoved([]));
    };
    useEffect(() => {
        if (auth === 'ok') readMoved();
    }, [auth]);

    useEffect(() => {
        if (auth !== 'ok') return;
        let alive = true;
        let tick = 0;
        let inFlight = false;
        const run = async () => {
            if (inFlight) return;
            inFlight = true;
            try {
                try {
                    const h = await api.health();
                    if (alive) {
                        setHealth(h);
                        setOffline(false);
                    }
                } catch {
                    if (alive) setOffline(true);
                }
                if (can('station.view') && Date.now() - readAvailability.current > 60_000) {
                    readAvailability.current = Date.now();
                    await api
                        .availability()
                        .then(noteAvailability)
                        .catch(() => undefined);
                }
                // Сессии и «пока не умеем» — вдвое реже: они меняются медленнее. Журнал — только тем, кому можно.
                if (tick++ % 2 === 0 && can('station.view')) {
                    try {
                        const sessions = await api.sessions();
                        const unsupported = can('journal.view') ? await api.unsupported().catch(() => [] as UnsupportedEntry[]) : [];
                        if (alive) setDebug({ sessions, unsupported, error: null });
                        await readStations();
                    } catch (e) {
                        if (e instanceof ApiError && e.status === 401) {
                            sessionStore.clear();
                            if (alive) setAuth('login');
                            return;
                        }
                        if (alive) setDebug({ sessions: null, unsupported: [], error: e as ApiError });
                    }
                }
            } finally {
                inFlight = false;
            }
        };
        pollNow.current = () => void run();
        void run();
        // С живым каналом перемены приходят сами: опрос — только страховка.
        const id = setInterval(run, feed.connected ? 15_000 : 3000);
        return () => {
            alive = false;
            clearInterval(id);
        };
    }, [auth, feed.connected]);

    // Выбранная колонка: запомненная, иначе та, что в сети, иначе первая подключённая.
    const live = (device: string): SessionSnapshot | undefined => debug.sessions?.find((s) => s.deviceTag === device && !s.closedAt);
    const approved = (stations ?? []).filter((entry) => entry.status === 'approved');
    const current =
        approved.find((entry) => entry.device === currentId) ?? approved.find((entry) => live(entry.device)) ?? approved[0] ?? null;
    const role = roleOf(health, current?.device);
    // Колонка помощника — со своим шлюзом: её голос идёт мимо компьютера, и в его записях колонок её может не быть.
    const helperStation = role.role === 'helper' ? role.station : undefined;
    const sections = useSections();
    // Раздел, которого этому входу не видно (или его плагин выключили), — первый из видных.
    const active = sections.find((entry) => entry.id === section) ?? sections[0];
    const home = sections.find((entry) => entry.id === 'home') ?? sections[0];
    // У помощника раздел колонки — её пульт во фрейме окна, через проход компьютера (docs/plans/desk.md, этап 4).
    const stationWindow =
        !VIA && role.role === 'helper' && opened.query.on === 'station' && active && active.id !== home?.id ? role.station : undefined;
    // Её перемены — на её шине: «добавь молоко», сказанное ей, шина компьютера не услышит. Её канал шлюз компьютера
    // пересказывает по сокету этой страницы (live.ts, gateway/src/placement/relay.rs) — своего потока к колонке у
    // страницы нет, и у окна колонки тоже: прежде их было по два на вкладку, и браузер, давая адресу шесть соединений
    // (HTTP/1.1), вставал. Виджеты, что читают колонку или зовут её плагины, перечитывают своё. Опрос компьютера
    // (pollNow) её перемены не трогают. В окне колонки (VIA) этого нет: тот пульт и так слышит колонку своим каналом.
    useStationLive(auth === 'ok' && !VIA && helperStation && can('station.view') ? helperStation.key : null, (name, subject, change) => {
        // Пакеты на колонке поменялись (или канал открылся заново): виджет плагина заново узнает, включён ли тот на ней.
        if ((name === 'packages.changed' && !runtimeOnly(change)) || name === 'hello') forgetStationPackages();
        notePackageChange(name, subject, change);
        // Шаг агента её мастерской — перемена на каждый вызов. Виджеты стола помощника её мастерскую не читают: это
        // слышит только плашка «Делаю виджет» (shell/agent-plaque.tsx, onStationChange).
        if (name === 'workshop.changed') return;
        setVersion((value) => value + 1);
    });
    // Ни одной колонки: ни подключённой, ни просящей подключиться. null — список не прочитан, тогда не знаем. У
    // помощника колонка есть всегда — та, с чьим шлюзом он связан.
    const noStation = !helperStation && stations !== null && stations.every((entry) => entry.status === 'blocked');
    const station = current ? (live(current.device) ?? debug.sessions?.find((s) => s.deviceTag === current.device)) : pickStation(debug);
    const drawing = drawingOf(current?.look, station?.device?.deviceModel ?? current?.deviceModel);
    // Как её слышно: по присутствию из живого канала, а без него — по голосовой сессии или по /health.
    const presence = current ? feed.presence[current.device] : undefined;
    const voiceSession = current && debug.sessions ? !!live(current.device) : health ? (health.station?.connected ?? false) : undefined;
    // У помощника колонку слышно по её шлюзу: голосовой сессии с компьютером у неё нет.
    const reach: Reach = helperStation ? (helperStation.reachable ? 'online' : 'offline') : reachOf(presence, voiceSession);
    const online = isReachable(reach);
    const lastSeen = presence?.lastSeen ?? (current && approved.length > 1 ? current.lastSeen : health?.station?.lastSeenAt);
    // Строка состояния: «на связи · компьютер помогает». Режим — на главной («Кто отвечает») и в «Настройки» → «Колонка».
    const status = noStation ? 'колонка не подключена' : [reachWords(reach, lastSeen), helpWords(role)].filter(Boolean).join(' · ');
    // Путь вопроса на дашборде: идущий запрос этой колонки, иначе последний законченный.
    const ofStation = feed.requests.filter((request) => !current || request.device === current.device);
    const trip = ofStation.find((request) => request.status === 'active') ?? ofStation.find((request) => request.status !== 'active');
    const waiting = (stations ?? []).find((entry) => needsSetup(entry) && !dismissed.includes(entry.device));
    const setupStation = setupId ? (stations ?? []).find((entry) => entry.device === setupId) : undefined;
    const later = (device: string): void => {
        const next = [...dismissed.filter((entry) => entry !== device), device];
        setDismissed(next);
        remember('glass-setup-later', next.join(','));
        setSetupId(null);
    };
    /** Встать туда, куда ведёт место: раздел, его вкладка, открытое во вкладке. Адрес не трогает. */
    const settle = (place: Place): void => {
        const tab = place.tab;
        const key = tabKey(place.section, place.query);
        setSection(place.section);
        if (tab) setTabs((old) => (old[key] === tab ? old : { ...old, [key]: tab }));
        setOpened({ item: tab ? place.item : undefined, query: place.query });
    };
    // Назад и вперёд браузера, ссылка, вставленная в адрес открытого пульта: пульт идёт за адресом.
    useEffect(() => {
        const follow = (): void => settle(parsePlace(location.hash) ?? { section: 'home', query: {} });
        window.addEventListener('hashchange', follow);
        window.addEventListener('popstate', follow);
        return () => {
            window.removeEventListener('hashchange', follow);
            window.removeEventListener('popstate', follow);
        };
    }, []);
    /** Перейти и записать адрес: новый шаг истории или замена текущего. */
    const navigate = (raw: Place, replace = false): void => {
        const place = normalizePlace(raw);
        settle(place);
        writePlace(place, replace);
    };
    /**
     * В раздел, а если названа вкладка — сразу на неё, а если элемент — сразу к нему. Несохранённое в открытом
     * (мастерская) сначала спрашивает: закрыть окно, выбрать раздел в доке — всё идёт сюда (shell/leave.ts).
     */
    const go = (next: string, tab?: string, item?: string, query: Record<string, string> = {}): void => {
        // Прежнее имя раздела («Мозг», «Дом», «Журнал») — сразу на его новое место (shell/links.ts).
        const named = normalizePlace({ section: next, tab, item, query });
        // Страница компьютера во фрейме пульта колонки (`?own=`) своих разделов не рисует: переход из неё — в самом
        // пульте вокруг, его адресом (источник тот же). Иначе нажатие меняло бы невидимое состояние фрейма.
        if (OWN_PAGE && window.parent !== window) {
            try {
                window.parent.location.hash = hashOf(named);
                return;
            } catch {
                // чужой источник — переход остаётся здесь
            }
        }
        // Пустая вкладка — раздел как есть, без запомненной (док открывает «Настройки» списком на телефоне).
        const remembered = tab === '' ? undefined : tabs[tabKey(named.section, query)];
        const place: Place = { section: named.section, tab: named.tab ?? remembered, item: named.tab ? named.item : undefined, query };
        if (!leaving()) return navigate(place);
        void mayLeave().then((ok) => ok && navigate(place));
    };
    const openHouse = (connector?: string): void => go('skills', 'connectors', connector);
    const signOut = (): void => {
        sessionStore.clear();
        setAuth('login');
        navigate({ section: 'home', query: {} }, true);
    };
    // Пульт колонки в окне помощника — пульт шлюза: белая рамка (via.ts).
    useEffect(() => applyRole(VIA ? 'gateway' : role.role), [role.role]);
    // Этап 4: компьютер помогает колонке — его окно и есть её пульт. Весь пульт идёт к колонке через проход компьютера;
    // своё у компьютера — «Настройки» → «Компьютер-помощник». `?local` оставляет пульт компьютера (разработчику).
    useEffect(() => {
        if (VIA || OWN_PAGE || LOCAL || auth !== 'ok' || role.role !== 'helper' || !role.station?.key) return;
        location.replace(`${location.pathname}?via=${encodeURIComponent(role.station.key)}${location.hash}`);
    }, [auth, role.role, role.station?.key]);
    useLeaveWhenNotHelping();

    if (auth === 'setup' || auth === 'login') {
        // Окно колонки у помощника вход не спрашивает: вход — у пульта компьютера; окно говорит, почему колонка не пустила.
        if (VIA) return <ViaRefused setup={auth === 'setup'} top={TOP_VIA} />;
        return (
            <AuthScreen
                mode={auth}
                accounts={accounts}
                needsCode={needsCode}
                onDone={() => {
                    // После первой настройки — сразу подключение колонки: без неё пульту нечего показать.
                    const first = auth === 'setup';
                    void learnWhoIAm().then((ok) => {
                        setAuth(ok ? 'ok' : 'login');
                        if (ok && first && can('stations.manage')) setConnectOpen(true);
                    });
                }}
            />
        );
    }
    if (auth === 'loading') return <div class="screen" />;

    const link: Place = { section, tab: tabs[tabKey(section, opened.query)], item: opened.item, query: opened.query };
    const panel: PanelState = {
        section,
        go,
        tabOf: <T extends string>(id: string, fallback: T): T => (tabs[id] as T | undefined) ?? fallback,
        setTab: (id, tab) => {
            if (id !== section) return setTabs((old) => ({ ...old, [id]: tab }));
            // Другая вкладка открытого раздела — тоже уход с несохранённого (мастерская — вкладка «Плагинов»).
            if (!leaving()) return navigate({ section: id, tab, query: {} });
            void mayLeave().then((ok) => ok && navigate({ section: id, tab, query: {} }));
        },
        link,
        openItem: (id, tab, item, query = {}, replace = false) => navigate({ section: id, tab, item: item ?? undefined, query }, replace),
        health,
        offline,
        debug,
        catalog,
        stations,
        feed,
        current,
        session: station,
        drawing,
        online,
        trip,
        version,
        recent: can('journal.view') ? ofStation.slice(0, 3) : undefined,
        banner: (
            <>
                {moved.map((entry) => (
                    <MovedBanner
                        key={entry.address}
                        moved={entry}
                        name={(stations ?? []).find((station) => station.device === entry.device)}
                        onReconnect={() => setReconnect(entry)}
                    />
                ))}
                {waiting && <SetupBanner station={waiting} onOpen={() => setSetupId(waiting.device)} />}
                {/* Срочный выпуск не прячется: «скрыть» у прошлой версии его не касается. */}
                {offered && (offered.critical || offered.version !== hiddenUpdate) && (
                    <UpdateBanner
                        version={offered.version}
                        critical={offered.critical}
                        auto={offered.auto}
                        onOpen={() => go('settings', 'updates')}
                        onHide={() => {
                            setHiddenUpdate(offered.version);
                            remember('glass-update-later', offered.version);
                        }}
                    />
                )}
            </>
        ),
        noStation,
        openConnect: () => setConnectOpen(true),
        openHouse,
        // Лист подключения — элемент вкладки «Подключения»: `#/skills/connectors/telegram` открывает его сразу.
        openConnector: section === 'skills' && tabs.skills === 'connectors' ? (opened.item ?? null) : null,
        connectorOpened: () => opened.item && navigate({ section: 'skills', tab: 'connectors', query: {} }, true),
        signOut,
        role,
        openAdd: () => setAddOpen(true),
    };
    // Страница компьютера внутри пульта колонки (`?own=`): только она, без дока и строки — её рамка уже стоит снаружи.
    if (OWN_PAGE)
        return (
            <PanelContext.Provider value={panel}>
                <OwnPage page={OWN_PAGE} />
            </PanelContext.Provider>
        );
    // Во фрейме окна — пульт колонки без своего дока; во всё окно (этап 4) — обычный, с доком и строкой состояния.
    const { Shell } = VIA && !TOP_VIA ? { Shell: WindowShell } : shellFor(theme.view);
    const nav = navOf(sections, role, dev);
    const activeNav = activeKey(link, role);
    const onNav = (entry: NavEntry): void => go(entry.section, entry.tab, undefined, entry.station ? { on: 'station' } : {});
    const name = ownName(health, role) ?? displayName(current, drawing);
    // Верхняя строка — колонка и её состояние; имя открывает выбор колонки.
    const line = (
        <>
            <button class="desk-top-pick desk-top-name" aria-haspopup="dialog" aria-expanded={pickerOpen} onClick={() => setPickerOpen(true)}>
                {name}
                <IconChevronDown />
            </button>
            <span class="desk-top-state">
                <span aria-hidden="true" class={dotClass(reach)} />
                {status}
            </span>
        </>
    );
    // Телефон: строка короче — имя колонки с точкой связи.
    const short = (
        <button
            class="desk-top-pick desk-top-short"
            aria-haspopup="dialog"
            aria-expanded={pickerOpen}
            aria-label={`Колонка «${name}», ${status}. Сменить колонку`}
            onClick={() => setPickerOpen(true)}
        >
            <span aria-hidden="true" class={dotClass(reach)} />
            <span class="desk-top-short-name">{name}</span>
            <IconChevronDown />
        </button>
    );
    const top: TopLine = { role: role.role, line, narrow: short };
    const places: SearchPlace[] = searchPlaces(role.role === 'helper', health?.placement?.role === 'station');
    const pickPlace = (entry: SearchPlace): void => {
        setSearchOpen(false);
        if (entry.act === 'say') return setSayOpen(true);
        if (entry.act === 'add') return setAddOpen(true);
        if (entry.act === 'theme') return theme.toggleMode();
        if (entry.place) go(entry.place.section, entry.place.tab, entry.place.item, entry.place.query ?? {});
    };
    const entryOf = (key: string): NavEntry | undefined => [...nav.left, ...nav.right].find((entry) => entry.key === key);
    const openedEntry = active && active.id !== home?.id ? entryOf(activeNav) : undefined;
    const scope: OpenWindow['scope'] = role.role === 'helper' && !VIA ? (link.query.on === 'station' ? 'station' : 'own') : undefined;
    const openWindow: OpenWindow | null =
        active && active.id !== home?.id
            ? {
                  // Окно — раздел и чей он; пункт дока («Плагины компьютера» или «Мастерская») — только подсветка и заголовок.
                  id: `${scope ?? ''}:${active.id}`,
                  key: activeNav,
                  title: openedEntry?.title ?? active.title,
                  scope,
                  badge: stationWindow ? (
                      <span class={`role-chip${stationWindow.reachable ? '' : ' is-off'}`}>
                          <span class="role-chip-dot" aria-hidden="true" />
                          колонка «{name}»
                      </span>
                  ) : undefined,
                  content: stationWindow ? (
                      // Элемент вкладки — тоже в адрес фрейма: плашка открывает черновик колонки в её «Плагинах».
                      <StationFrame
                          key={`${active.id}/${link.tab ?? ''}/${link.item ?? ''}`}
                          station={stationWindow.key}
                          section={active.id}
                          tab={link.tab}
                          item={link.item}
                      />
                  ) : (
                      <Boundary key={active.id} name={`раздел ${active.id}`} fallback={<SectionFailed title={active.title} />}>
                          <active.Screen />
                      </Boundary>
                  ),
                  onClose: () => go('home'),
              }
            : null;

    return (
        <PanelContext.Provider value={panel}>
            <Shell
                sections={sections}
                nav={nav}
                active={activeNav}
                onNav={onNav}
                top={top}
                desk={
                    home && (
                        <Boundary key={home.id} name={`раздел ${home.id}`} fallback={<SectionFailed title={home.title} />}>
                            <home.Screen />
                        </Boundary>
                    )
                }
                window={openWindow}
                station={
                    <StationButton
                        name={name}
                        drawing={drawing}
                        status={status}
                        reach={reach}
                        open={pickerOpen}
                        onOpen={() => setPickerOpen(true)}
                    />
                }
                actions={
                    <>
                        <button class="icon-btn glass search-btn" aria-label="Найти в пульте (⌘K)" title="Найти (⌘K)" onClick={() => setSearchOpen(true)}>
                            <IconSearch size={16} />
                        </button>
                        <Slot name="header.actions" />
                        <AccountMenu onLook={() => go('settings', 'look')} onAccess={() => go('settings', 'access')} onSignOut={signOut} />
                    </>
                }
                talk={{ disabled: !can('station.say'), onTalk: () => setSayOpen(true) }}
                notice={
                    <>
                        {offline && (
                            <div class="banner-offline" role="status">
                                DuoWave не отвечает — пульт ждёт соединения…
                            </div>
                        )}
                        {TOP_VIA && <ViaNotice />}
                    </>
                }
            />

            {searchOpen && <SearchPalette places={places} onPick={pickPlace} onClose={() => setSearchOpen(false)} />}
            {addOpen && (
                <AddWidgetPanel onClose={() => setAddOpen(false)}>
                    <WidgetAgent
                        onPut={(key) => {
                            // Поставленный виджет — на стол сразу, ещё до того, как пульт подхватит новый плагин.
                            deskActions(role.role).add(key);
                            setAddOpen(false);
                        }}
                    />
                </AddWidgetPanel>
            )}
            {sayOpen && <SaySheet health={health} debug={debug} onClose={() => setSayOpen(false)} />}
            {pickerOpen && (
                <StationsPicker
                    stations={stations}
                    current={current}
                    sessions={debug.sessions}
                    presence={feed.presence}
                    onPick={(device) => {
                        setCurrentId(device);
                        remember('glass-station', device);
                    }}
                    onSetup={(device) => {
                        setPickerOpen(false);
                        setSetupId(device);
                    }}
                    onChanged={() => void readStations()}
                    onClose={() => setPickerOpen(false)}
                    onOpenBrain={() => {
                        setPickerOpen(false);
                        void readStations();
                        go(...settingsPlace('brain'));
                    }}
                />
            )}
            {(connectOpen || reconnect) && (
                <ConnectSheet
                    initial={reconnect ? { address: reconnect.address, gateway: reconnect.now } : undefined}
                    onClose={() => {
                        setConnectOpen(false);
                        setReconnect(null);
                        void readStations();
                        readMoved();
                    }}
                    onConnected={() => {
                        // Первый запуск (этап 5): имя и вид — позже, плашкой на главной, без отдельного шага.
                        setConnectOpen(false);
                        setReconnect(null);
                        void readStations();
                        readMoved();
                    }}
                    onOpenBrain={() => {
                        setConnectOpen(false);
                        setReconnect(null);
                        void readStations();
                        readMoved();
                        go(...settingsPlace('brain'));
                    }}
                />
            )}
            {setupStation && (
                <SetupSheet
                    key={setupStation.device}
                    station={setupStation}
                    onClose={() => setSetupId(null)}
                    onChanged={() => void readStations()}
                    onLater={() => later(setupStation.device)}
                />
            )}
        </PanelContext.Provider>
    );
}

/** Раздел упал при отрисовке: остальной пульт работает, раздел говорит, что случилось. */
function SectionFailed({ title }: { title: string }) {
    return (
        <section class="card glass" role="alert">
            <p class="t-meta m-0">
                Раздел «{title}» не открылся — ошибка в его коде (подробности в консоли браузера). Остальное работает; обновите
                страницу, чтобы попробовать снова.
            </p>
        </section>
    );
}
