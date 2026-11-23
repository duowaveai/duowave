// backend.js приложения на витрине сайта: в своём воркере, как в шлюзе — в своей QuickJS. Ни DOM, ни страницы; `host` —
// из backend.ts. Сеть воркеру закрыта правилом его ответа (`connect-src 'none'`, заголовки сайта), а проверка
// (check.ts) не пускает код, который ходит мимо хозяина. Хозяин (host.ts) говорит с воркером сообщениями ниже.

import { BackendSetup, Json, Writes, callBackend, makeHost } from './backend';

export type ToWorker =
    | { type: 'init'; backend: string; calls: string[]; setup: BackendSetup }
    | { type: 'call'; seq: number; fn: string; input: unknown };

export type FromWorker =
    | { type: 'ready' }
    | { type: 'failed'; message: string }
    | { type: 'answer'; seq: number; ok: boolean; value?: Json; error?: string; writes: Writes }
    | { type: 'log'; level: string; text: string };

const post = (message: FromWorker): void => (self as unknown as Worker).postMessage(message);

let module: Record<string, unknown> | null = null;
let calls: string[] = [];
let taken: () => Writes = () => [];
// Вызовы — по одному, как у шлюза: следующий ждёт, пока предыдущий ответит.
let queue: Promise<unknown> = Promise.resolve();

self.addEventListener('message', (event: MessageEvent<ToWorker>) => {
    const message = event.data;
    if (message.type === 'init' && !module) {
        const made = makeHost(message.setup, (level, text) => post({ type: 'log', level, text }));
        taken = made.taken;
        calls = message.calls;
        Object.defineProperty(globalThis, 'host', { value: made.host, configurable: false, writable: false });
        Object.defineProperty(globalThis, 'console', { value: made.console, configurable: true, writable: false });
        import(/* @vite-ignore */ message.backend).then(
            (loaded: Record<string, unknown>) => {
                module = loaded;
                post({ type: 'ready' });
            },
            (error: unknown) => post({ type: 'failed', message: error instanceof Error ? error.message : String(error) }),
        );
    } else if (message.type === 'call') {
        const { seq, fn, input } = message;
        queue = queue.then(async () => {
            if (!module) {
                post({ type: 'answer', seq, ok: false, error: 'backend.js ещё не загружен', writes: [] });
                return;
            }
            try {
                const value = await callBackend(module, calls, fn, input);
                post({ type: 'answer', seq, ok: true, value, writes: taken() });
            } catch (error) {
                post({ type: 'answer', seq, ok: false, error: error instanceof Error ? error.message : String(error), writes: taken() });
            }
        });
    }
});
