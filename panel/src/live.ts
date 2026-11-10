import { useEffect, useRef, useState } from 'preact/hooks';

import { LiveMessage, RequestView, StationPresence, liveHeaders } from './api';
import { VIA } from './via';

// Живой канал шлюза (/api/v1/live/ws, docs/plans/desk-after-station.md, А): присутствие колонок, запросы на ходу и имена
// перемен, после которых пульт перечитывает своё. Это WebSocket: одно соединение на вкладку. Заголовок входа сокету не
// передать, а токену в адресе не место — пульт сперва берёт одноразовый билет (POST /api/v1/live/ticket) и открывает
// сокет с ним. У помощника по тому же сокету идёт и канал его колонки: шлюз компьютера держит к ней одно соединение и
// пересказывает её сообщения с пометкой колонки (`station`, gateway/src/placement/relay.rs). Своих потоков к колонке
// пульт не открывает: прежде их было по два на вкладку, и шесть соединений, что браузер даёт адресу, кончались — пульт
// вставал целиком. Окно колонки (фрейм `?via=`) своего канала не держит: страница помощника передаёт ему сообщения
// колонки словом STATION_LIVE.

/** Сколько запросов держать: идущие и последние законченные. */
const KEPT = 40;
/** Законченных в hello, как у шлюза (live/mod.rs, HELLO_RECENT). */
const HELLO_RECENT = 10;
/** Шлюз шлёт отметку раз в 20 с: тишина дольше — соединение умерло (телефон уснул, сеть сменилась). */
const SILENCE_MS = 45_000;
/** Пауза перед новой попыткой после обрыва: 1, 2, 5, 10 с. */
const BACKOFF_MS = [1000, 2000, 5000, 10_000];
/** Шлюз закрыл сокет сам (его срок, 15 минут): открыть снова почти сразу. */
const AGAIN_MS = 300;

/**
 * Слово страницы помощника окну колонки (фрейму `?via=`, shell/station-window.tsx): `message` — что сказал канал
 * колонки; без него — канал потерян. Рядом — слова шита (ui/sheet.tsx, SHEET_MESSAGE) и Esc (shell/window.tsx).
 */
export const STATION_LIVE = 'duowave:station-live';
/** Слово окна колонки странице: «слушаю» — страница отвечает тем, что знает о колонке сейчас (hello). */
export const STATION_READY = 'duowave:station-ready';

/** Пульт колонки во фрейме окна помощника: её канал ему пересказывает страница. */
const framed = !!VIA && window.parent !== window;

export interface LiveState {
    /** Канал открыт: то, что он показывает, — текущее. */
    connected: boolean;
    /** Часы шлюза минус наши, мс: идущий запрос меряется по шлюзу. */
    skew: number;
    presence: Record<string, StationPresence>;
    /** Новые сначала. */
    requests: RequestView[];
}

const EMPTY: LiveState = { connected: false, skew: 0, presence: {}, requests: [] };

const byStart = (a: RequestView, b: RequestView): number => b.startedAt.localeCompare(a.startedAt);

/** Запрос в список: замена по id, новые сначала, старые законченные — за край. */
export function mergeRequest(list: RequestView[], request: RequestView): RequestView[] {
    return [request, ...list.filter((entry) => entry.id !== request.id)].sort(byStart).slice(0, KEPT);
}

/** Состояние канала после сообщения: hello — всё заново, запрос и присутствие — поверх. */
function reduce(state: LiveState, message: LiveMessage): LiveState {
    switch (message.type) {
        case 'hello':
            return {
                connected: true,
                skew: Date.parse(message.serverTime) - Date.now(),
                presence: Object.fromEntries(message.presence.map((entry) => [entry.device, entry])),
                requests: [...message.active, ...message.recent].sort(byStart).slice(0, KEPT),
            };
        case 'request':
            return { ...state, requests: mergeRequest(state.requests, message.request) };
        case 'presence':
            return { ...state, presence: { ...state.presence, [message.presence.device]: message.presence } };
        default:
            return state;
    }
}

/** Адрес сокета с билетом: тот же хост, ws или wss — как у страницы. */
function socketUrl(ticket: string): string {
    return `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/v1/live/ws?ticket=${encodeURIComponent(ticket)}`;
}

/** Что делать с тем, что говорит канал. */
interface Channel {
    /** Сообщение канала; hello — после каждого подключения. */
    onMessage: (message: LiveMessage) => void;
    /** Канал закрылся или оборвался: то, что он показывал, уже не текущее. */
    onLost: () => void;
}

/**
 * Держать сокет своего шлюза открытым: билет, сокет, сообщения. Шлюз закрыл его сам (раз в 15 минут, с новой проверкой
 * входа) — открыть сразу; оборвался — с паузой; тишину дольше отметок считать обрывом. Билет не дали (401/403) —
 * молчать: вход решает остальной пульт. Возвращает, как закрыть.
 */
function keepOpen(channel: Channel): () => void {
    let stopped = false;
    let socket: WebSocket | undefined;
    let ticketing: AbortController | undefined;
    let failures = 0;
    let timer = 0;
    let lastData = Date.now();
    /** Пришло hello: канал открыт. */
    let open = false;
    /** Идёт попытка — билет или сокет: вторую рядом не начинать. */
    let busy = false;

    const ended = (normal: boolean): void => {
        socket = undefined;
        open = false;
        busy = false;
        if (stopped) return;
        channel.onLost();
        if (!normal) failures += 1;
        const wait = failures ? BACKOFF_MS[Math.min(failures, BACKOFF_MS.length) - 1] : AGAIN_MS;
        timer = window.setTimeout(() => void connect(), wait);
    };

    /** Сокет без закрытия по правилам: обработчики сняты, чтобы он не сказал о себе после. */
    const forget = (dead: WebSocket): void => {
        dead.onclose = null;
        dead.onmessage = null;
        dead.onerror = null;
        try {
            dead.close();
        } catch {
            // уже закрыт
        }
    };

    const connect = async (): Promise<void> => {
        window.clearTimeout(timer);
        if (busy || stopped) return;
        busy = true;
        lastData = Date.now();
        let ticket: string;
        try {
            ticketing = new AbortController();
            const response = await fetch('/api/v1/live/ticket', { method: 'POST', headers: liveHeaders(), signal: ticketing.signal });
            if (response.status === 401 || response.status === 403) {
                busy = false;
                if (!stopped) channel.onLost();
                return;
            }
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            ticket = ((await response.json()) as { message: { ticket: string } }).message.ticket;
        } catch {
            ended(false);
            return;
        }
        if (stopped) return;
        const ws = new WebSocket(socketUrl(ticket));
        socket = ws;
        ws.onmessage = (event) => {
            lastData = Date.now();
            let message: LiveMessage | { type: 'ping' };
            try {
                message = JSON.parse(String(event.data)) as LiveMessage | { type: 'ping' };
            } catch {
                return; // испорченное сообщение пропускаем: следующее придёт целым
            }
            if (message.type === 'ping') return;
            failures = 0;
            if (message.type === 'hello') open = true;
            channel.onMessage(message);
        };
        ws.onclose = (event) => {
            if (socket !== ws) return;
            // 1000 — шлюз закрыл сам: срок вышел или он отпустил отставший сокет.
            ended(event.code === 1000);
        };
    };

    // Тишина дольше отметок — соединение мертво, хоть браузер этого и не заметил; так же — подключение, что повисло.
    const watchdog = window.setInterval(() => {
        if (!(open || busy) || Date.now() - lastData <= SILENCE_MS) return;
        const dead = socket;
        if (dead) {
            forget(dead);
            ended(false);
        } else ticketing?.abort();
    }, 10_000);
    // Вкладка вернулась: не ждать паузы, подключиться сейчас.
    const onVisible = (): void => {
        if (document.visibilityState === 'visible' && !open && !busy && !stopped) {
            failures = 0;
            void connect();
        }
    };
    document.addEventListener('visibilitychange', onVisible);
    void connect();
    return () => {
        stopped = true;
        ticketing?.abort();
        if (socket) forget(socket);
        socket = undefined;
        window.clearTimeout(timer);
        window.clearInterval(watchdog);
        document.removeEventListener('visibilitychange', onVisible);
    };
}

/**
 * Окно колонки во фрейме: её канал ему пересказывает страница помощника (STATION_LIVE) — с проверкой, что слово от
 * своей страницы и с этого же адреса. Сразу говорит ей «слушаю» (STATION_READY): страница отвечает тем, что знает.
 */
function fromHelper(channel: Channel): () => void {
    const onMessage = (event: MessageEvent): void => {
        if (event.source !== window.parent || event.origin !== location.origin) return;
        const data = event.data as { type?: unknown; message?: unknown } | null;
        if (data?.type !== STATION_LIVE) return;
        if (data.message && typeof data.message === 'object') channel.onMessage(data.message as LiveMessage);
        else channel.onLost();
    };
    window.addEventListener('message', onMessage);
    try {
        window.parent.postMessage({ type: STATION_READY }, location.origin);
    } catch {
        // страницы уже нет
    }
    return () => window.removeEventListener('message', onMessage);
}

/**
 * Живой канал: при отказе во входе молчит — вход решает остальной пульт. `onChanged` — имя перемены (и 'hello' после
 * каждого подключения: что поменялось, пока канала не было, пульт перечитывает), чья она (`subject`, id пакета) и что
 * случилось (`change`). Сообщения колонки помощника идут мимо: их слушают useStationLive, onStationChange и окно колонки
 * (onStationMessage). В окне колонки (`?via=`) канал — её: во фрейме его пересказывает страница помощника, а открытое
 * само по себе окно берёт его из своего сокета к компьютеру.
 */
export function useLive(enabled: boolean, onChanged?: (name: string, subject?: string, change?: string) => void): LiveState {
    const [state, setState] = useState<LiveState>(EMPTY);
    const changed = useRef(onChanged);
    changed.current = onChanged;

    useEffect(() => {
        if (!enabled) return;
        const heard = (message: LiveMessage): void => {
            switch (message.type) {
                case 'hello':
                    setState(reduce(EMPTY, message));
                    changed.current?.('hello');
                    return;
                case 'request':
                case 'presence':
                    setState((now) => reduce(now, message));
                    return;
                case 'changed':
                    changed.current?.(message.name, message.subject, message.change);
                    return;
                case 'station':
                    if (!VIA) heardStation(message.station, message.message ?? null);
                    return;
            }
        };
        const lost = (): void => setState((now) => (now.connected ? { ...now, connected: false } : now));
        // Окно колонки, открытое само по себе (не во фрейме): из сокета компьютера — только её канал.
        const ofVia = (message: LiveMessage): void => {
            if (message.type !== 'station' || message.station !== VIA) return;
            if (message.message) heard(message.message);
            else lost();
        };
        const close = !VIA
            ? keepOpen({
                  onMessage: heard,
                  onLost: () => {
                      lost();
                      loseStations();
                  },
              })
            : framed
              ? fromHelper({ onMessage: heard, onLost: lost })
              : keepOpen({ onMessage: ofVia, onLost: lost });
        return () => {
            close();
            setState(EMPTY);
            if (!VIA) loseStations();
        };
    }, [enabled]);

    return enabled ? state : EMPTY;
}

// ─── Колонка помощника ─────────────────────────────────────────────────────────────────────────────────────────────

/** Что страница знает о канале каждой колонки по сокету компьютера: для окна колонки, что откроется позже. */
const stationFeeds = new Map<string, LiveState>();

/** Сообщение канала колонки (null — канал потерян) — окнам колонки на странице. */
type StationMessageListener = (station: string, message: LiveMessage | null) => void;

const messageListeners = new Set<StationMessageListener>();

/** Слушать сообщения каналов колонок, как их пересказал шлюз компьютера: окно колонки передаёт их своему фрейму. */
export function onStationMessage(listener: StationMessageListener): () => void {
    messageListeners.add(listener);
    return () => {
        messageListeners.delete(listener);
    };
}

/** Что известно о канале колонки сейчас — сообщением hello, для окна, что только открылось. null — канала к ней нет. */
export function stationHello(station: string): LiveMessage | null {
    const state = stationFeeds.get(station);
    if (!state?.connected) return null;
    return {
        type: 'hello',
        serverTime: new Date(Date.now() + state.skew).toISOString(),
        presence: Object.values(state.presence),
        active: state.requests.filter((request) => request.status === 'active'),
        recent: state.requests.filter((request) => request.status !== 'active').slice(0, HELLO_RECENT),
    };
}

/** Сообщение канала колонки из сокета компьютера: в её состояние, окнам колонки и тем, кто слушает её перемены. */
function heardStation(station: string, message: LiveMessage | null): void {
    if (message) stationFeeds.set(station, reduce(stationFeeds.get(station) ?? EMPTY, message));
    else if (!stationFeeds.delete(station)) return;
    for (const listener of [...messageListeners]) listener(station, message);
    if (message?.type === 'hello') stationChanged('hello', station);
    else if (message?.type === 'changed') stationChanged(message.name, station, message.subject, message.change);
}

/** Сокет компьютера потерян: каналов колонок через него тоже нет. */
function loseStations(): void {
    for (const station of [...stationFeeds.keys()]) heardStation(station, null);
}

/** Перемена колонки помощника: её имя (или 'hello' — канал открылся заново), ключ колонки, чья она и что случилось. */
type StationListener = (name: string, station: string, subject?: string, change?: string) => void;

const stationListeners = new Set<StationListener>();

/**
 * Слушать перемены колонки помощника: о них говорит её канал, который пересказывает шлюз компьютера. Плашка «Делаю
 * виджет» по `workshop.changed` перечитывает работу мастерской колонки. Возвращает, как перестать.
 */
export function onStationChange(listener: StationListener): () => void {
    stationListeners.add(listener);
    return () => {
        stationListeners.delete(listener);
    };
}

/** Перемена колонки помощника — всем, кто её слушает: обработчику useStationLive и onStationChange. */
export function stationChanged(name: string, station: string, subject?: string, change?: string): void {
    for (const listener of [...stationListeners]) listener(name, station, subject, change);
}

/**
 * Перемены колонки помощника (docs/plans/desk.md, этап 4). Шлюз колонки — на ней самой: «добавь молоко», сказанное ей,
 * и виджет, который её мастерская делает по её голосу, — перемены на её шине. Свой поток к ней пульт не открывает: её
 * канал пересказывает шлюз компьютера по сокету страницы (useLive). `station` — ключ колонки; null — не слушать: у
 * шлюза, в самом окне колонки, без входа. `onChanged` — имя её перемены ('hello' — канал открылся заново), чья она и
 * что случилось. Те же имена слышат и onStationChange.
 */
export function useStationLive(station: string | null, onChanged: (name: string, subject?: string, change?: string) => void): void {
    const changed = useRef(onChanged);
    changed.current = onChanged;
    const key = useRef(station);
    key.current = station;

    useEffect(
        () =>
            onStationChange((name, from, subject, change) => {
                if (key.current && from === key.current) changed.current(name, subject, change);
            }),
        [],
    );
}
