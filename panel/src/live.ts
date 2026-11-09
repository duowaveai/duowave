import { useEffect, useRef, useState } from 'preact/hooks';

import { LiveMessage, RequestView, StationPresence, liveHeaders } from './api';

// Живой канал шлюза (/api/v1/live): присутствие колонок, запросы на ходу и имена перемен, после которых пульт
// перечитывает своё. Читается fetch'ем с токеном входа — EventSource заголовок не передаёт.

/** Сколько запросов держать: идущие и последние законченные. */
const KEPT = 40;
/** Шлюз шлёт отметку раз в 20 с: тишина дольше — соединение умерло (телефон уснул, сеть сменилась). */
const SILENCE_MS = 45_000;
/** Пауза перед новой попыткой после обрыва: 1, 2, 5, 10 с. */
const BACKOFF_MS = [1000, 2000, 5000, 10_000];

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

/** Сообщения из тела ответа: блоки SSE, разделённые пустой строкой; строки-комментарии (отметки) только будят. */
async function* messagesOf(body: ReadableStream<Uint8Array>, onData: () => void): AsyncIterable<LiveMessage> {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        onData();
        buffer += decoder.decode(value, { stream: true });
        let end: number;
        while ((end = buffer.indexOf('\n\n')) >= 0) {
            const block = buffer.slice(0, end);
            buffer = buffer.slice(end + 2);
            const data = block
                .split('\n')
                .filter((line) => line.startsWith('data:'))
                .map((line) => line.slice(5).trimStart())
                .join('\n');
            if (!data) continue;
            try {
                yield JSON.parse(data) as LiveMessage;
            } catch {
                // испорченный блок пропускаем: следующий придёт целым
            }
        }
    }
}

/**
 * Живой канал: открывается снова, когда шлюз его закрыл (раз в 15 минут, с новой проверкой входа) или он оборвался;
 * при 401/403 молчит — вход решает остальной пульт. `onChanged` — имя перемены (и 'hello' после каждого
 * подключения: что поменялось, пока канала не было, пульт перечитывает).
 */
export function useLive(enabled: boolean, onChanged?: (name: string) => void): LiveState {
    const [state, setState] = useState<LiveState>(EMPTY);
    const changed = useRef(onChanged);
    changed.current = onChanged;

    useEffect(() => {
        if (!enabled) return;
        let stopped = false;
        let attempt: AbortController | undefined;
        let failures = 0;
        let timer = 0;
        let lastData = Date.now();
        /** Канал открыт (пришло hello) и попытка идёт: вторую рядом не открывать. */
        let open = false;
        let busy = false;

        const apply = (message: LiveMessage): void => {
            switch (message.type) {
                case 'hello':
                    open = true;
                    setState({
                        connected: true,
                        skew: Date.parse(message.serverTime) - Date.now(),
                        presence: Object.fromEntries(message.presence.map((entry) => [entry.device, entry])),
                        requests: [...message.active, ...message.recent].sort(byStart).slice(0, KEPT),
                    });
                    changed.current?.('hello');
                    return;
                case 'request':
                    setState((now) => ({ ...now, requests: mergeRequest(now.requests, message.request) }));
                    return;
                case 'presence':
                    setState((now) => ({ ...now, presence: { ...now.presence, [message.presence.device]: message.presence } }));
                    return;
                case 'changed':
                    changed.current?.(message.name);
                    return;
            }
        };

        const connect = async (): Promise<void> => {
            window.clearTimeout(timer);
            if (busy || stopped) return;
            busy = true;
            attempt = new AbortController();
            lastData = Date.now();
            let denied = false;
            try {
                const response = await fetch('/api/v1/live', { headers: liveHeaders(), signal: attempt.signal });
                if (response.status === 401 || response.status === 403) denied = true;
                else if (!response.ok || !response.body) throw new Error(`HTTP ${response.status}`);
                else {
                    for await (const message of messagesOf(response.body, () => (lastData = Date.now()))) {
                        failures = 0;
                        apply(message);
                    }
                }
            } catch {
                failures += 1;
            }
            open = false;
            busy = false;
            if (stopped) return;
            setState((now) => (now.connected ? { ...now, connected: false } : now));
            if (denied) return;
            // Шлюз закрыл канал сам (его срок) — сразу снова; оборвался — с паузой.
            timer = window.setTimeout(() => void connect(), failures ? BACKOFF_MS[Math.min(failures, BACKOFF_MS.length) - 1] : 300);
        };

        // Тишина дольше отметок — соединение мертво, хоть браузер этого и не заметил.
        const watchdog = window.setInterval(() => {
            if (open && Date.now() - lastData > SILENCE_MS) attempt?.abort();
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
            attempt?.abort();
            window.clearTimeout(timer);
            window.clearInterval(watchdog);
            document.removeEventListener('visibilitychange', onVisible);
            setState(EMPTY);
        };
    }, [enabled]);

    return enabled ? state : EMPTY;
}
