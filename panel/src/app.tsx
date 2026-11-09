import { useEffect, useRef, useState } from 'preact/hooks';

import { AccessCatalog, ApiError, Health, MovedGateway, SessionSnapshot, Station, UnsupportedEntry, api, can, sessionStore } from './api';
import { noteAvailability } from './availability';
import { settingsPlace } from './builtin';
import { IconMoon, IconSun } from './icons';
import { useLive } from './live';
import { isReachable, reachLine, reachOf } from './presence';
import { AuthScreen } from './screens/auth';
import { SaySheet } from './screens/say';
import { ConnectSheet } from './screens/connect';
import { SetupBanner, SetupSheet, StationButton, StationsPicker, displayName, needsSetup } from './screens/stations';
import { DebugState, PanelContext, PanelState } from './shell/context';
import { Place, parsePlace, writePlace } from './shell/links';
import { useSections } from './shell/sections';
import { shellFor } from './shell/shells';
import { Boundary, Slot } from './shell/slot';
import { useTheme } from './shell/theme';
import { drawingOf } from './stations';
import { MovedBanner } from './ui/moved-banner';
import { UpdateBanner } from './ui/update-banner';
import { readUpdateStatus, useOfferedVersion } from './updates';
import { syncPackages } from './plugins/loader';

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

const MODE_WORDS: Record<string, string> = { relay: 'Яндекс', gateway: 'шлюз', own: 'свой' };

/** Где работает шлюз, для шапки: «шлюз на колонке · ПК на связи». На компьютере без колонки — ничего. */
function placementLine(health: Health | null): string | null {
    const placement = health?.placement;
    if (!placement) return null;
    if (placement.role === 'station') {
        const home = placement.homeServer;
        // Адрес ПК известен, но помощь выключена («ПК помогает колонке»): колонка одна, и это не беда.
        return home && home.enabled !== false ? `шлюз на колонке · ПК ${home.state === 'up' ? 'на связи' : 'не на связи'}` : 'шлюз на колонке';
    }
    const station = placement.station;
    if (station && station.placement !== 'pc') return `шлюз на колонке · ${station.reachable ? 'на связи' : 'не отвечает'}`;
    return null;
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
    const [section, setSection] = useState(() => boot?.section ?? 'home');
    /** Вкладки внутри разделов: раздел помнит, где его оставили, а шиты дашборда ведут прямо к нужной. */
    const [tabs, setTabs] = useState<Record<string, string>>(() => (boot?.tab ? { [boot.section]: boot.tab } : {}));
    /** Что открыто во вкладке по адресу (лист подключения, карточка персонажа) и параметры ссылки. */
    const [opened, setOpened] = useState<{ item?: string; query: Record<string, string> }>(() => ({
        item: boot?.tab ? boot.item : undefined,
        query: boot?.query ?? {},
    }));
    const [sayOpen, setSayOpen] = useState(false);
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
    // Живой канал: присутствие колонок и запросы на ходу; перемена где-то ещё — перечитать сейчас, не ждать опроса.
    const feed = useLive(auth === 'ok' && can('station.view'), (name) => {
        // Новая версия или канал открылся заново (пока его не было, шлюз мог найти версию или поставить её).
        if (name === 'update.available' || name === 'hello') void readUpdateStatus();
        if (name === 'packages.changed' || name === 'hello') setPackagesVersion((value) => value + 1);
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
    const offered = useOfferedVersion();
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
    // Ни одной колонки: ни подключённой, ни просящей подключиться. null — список не прочитан, тогда не знаем.
    const noStation = stations !== null && stations.every((entry) => entry.status === 'blocked');
    const current =
        approved.find((entry) => entry.device === currentId) ?? approved.find((entry) => live(entry.device)) ?? approved[0] ?? null;
    const station = current ? (live(current.device) ?? debug.sessions?.find((s) => s.deviceTag === current.device)) : pickStation(debug);
    const drawing = drawingOf(current?.look, station?.device?.deviceModel ?? current?.deviceModel);
    // Как её слышно: по присутствию из живого канала, а без него — по голосовой сессии или по /health.
    const presence = current ? feed.presence[current.device] : undefined;
    const voiceSession = current && debug.sessions ? !!live(current.device) : health ? (health.station?.connected ?? false) : undefined;
    const reach = reachOf(presence, voiceSession);
    const online = isReachable(reach);
    const lastSeen = presence?.lastSeen ?? (current && approved.length > 1 ? current.lastSeen : health?.station?.lastSeenAt);
    const placementWords = placementLine(health);
    const status = noStation
        ? 'колонка не подключена'
        : [reachLine(reach, MODE_WORDS[health?.panelMode ?? health?.mode ?? 'gateway'] ?? 'шлюз', lastSeen), placementWords]
              .filter(Boolean)
              .join(' · ');
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
        setSection(place.section);
        if (tab) setTabs((old) => (old[place.section] === tab ? old : { ...old, [place.section]: tab }));
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
    const navigate = (place: Place, replace = false): void => {
        settle(place);
        writePlace(place, replace);
    };
    /** В раздел, а если названа вкладка — сразу на неё, а если элемент — сразу к нему. */
    const go = (next: string, tab?: string, item?: string, query: Record<string, string> = {}): void =>
        navigate({ section: next, tab: tab ?? tabs[next], item: tab ? item : undefined, query });
    const openHouse = (connector?: string): void => go('house', 'connectors', connector);
    const signOut = (): void => {
        sessionStore.clear();
        setAuth('login');
        navigate({ section: 'home', query: {} }, true);
    };
    const sections = useSections();

    if (auth === 'setup' || auth === 'login')
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
    if (auth === 'loading') return <div class="screen" />;

    const link: Place = { section, tab: tabs[section], item: opened.item, query: opened.query };
    const panel: PanelState = {
        section,
        go,
        tabOf: <T extends string>(id: string, fallback: T): T => (tabs[id] as T | undefined) ?? fallback,
        setTab: (id, tab) => {
            if (id === section) navigate({ section: id, tab, query: {} });
            else setTabs((old) => ({ ...old, [id]: tab }));
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
                {offered && offered !== hiddenUpdate && (
                    <UpdateBanner
                        version={offered}
                        onOpen={() => go('house', 'gateway')}
                        onHide={() => {
                            setHiddenUpdate(offered);
                            remember('glass-update-later', offered);
                        }}
                    />
                )}
            </>
        ),
        noStation,
        openConnect: () => setConnectOpen(true),
        openHouse,
        // Лист подключения — элемент вкладки «Коннекторы»: `#/house/connectors/telegram` открывает его сразу.
        openConnector: section === 'house' && tabs.house === 'connectors' ? (opened.item ?? null) : null,
        connectorOpened: () => opened.item && navigate({ section: 'house', tab: 'connectors', query: {} }, true),
        signOut,
    };
    // Раздел, которого этому входу не видно (или его плагин выключили), — первый из видных.
    const active = sections.find((entry) => entry.id === section) ?? sections[0];
    const { Shell } = shellFor(theme.theme.layout);

    return (
        <PanelContext.Provider value={panel}>
            <Shell
                sections={sections}
                active={active?.id ?? ''}
                onGo={(id) => go(id)}
                station={
                    <StationButton
                        name={displayName(current, drawing)}
                        drawing={drawing}
                        status={status}
                        reach={reach}
                        open={pickerOpen}
                        onOpen={() => setPickerOpen(true)}
                    />
                }
                actions={
                    <>
                        <Slot name="header.actions" />
                        <button
                            aria-label={theme.mode === 'dark' ? 'Светлый режим' : 'Тёмный режим'}
                            class="icon-btn glass"
                            onClick={theme.toggleMode}
                        >
                            {theme.mode === 'dark' ? <IconSun /> : <IconMoon />}
                        </button>
                    </>
                }
                talk={{ disabled: !can('station.say'), onTalk: () => setSayOpen(true) }}
                notice={
                    offline && (
                        <div class="banner-offline" role="status">
                            Шлюз недоступен — панель ждёт соединения…
                        </div>
                    )
                }
            >
                {active && (
                    <Boundary key={active.id} name={`раздел ${active.id}`} fallback={<SectionFailed title={active.title} />}>
                        <active.Screen />
                    </Boundary>
                )}
            </Shell>

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
                    onConnected={(device) => {
                        setConnectOpen(false);
                        setReconnect(null);
                        void readStations();
                        readMoved();
                        setSetupId(device);
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
