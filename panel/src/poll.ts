// Опрос без наложений (docs/plans/desk-after-station.md, А). Виджеты стола читают колонку раз в несколько секунд, и
// раньше `setInterval` не ждал прежнего ответа, а каждая перемена в пульте (panel.version) запускала опрос заново: проход
// к колонке ждал до 20 с, и запросы копились, занимая те немногие соединения, что браузер даёт адресу. Здесь:
// - новый запрос не начинается, пока идёт прежний;
// - уход со стола и смена того, что читается (`deps`), обрывают идущий запрос;
// - перемена пульта перечитывает не чаще раза в `settle` мс и таймер опроса не перезапускает.

import { useEffect, useRef } from 'preact/hooks';

/** Перечитывать по перемене пульта не чаще. */
export const SETTLE_MS = 3000;

/** Чтение: `signal` обрывает его; ответ после обрыва не нужен — проверяйте `signal.aborted`. */
export type Read = (signal: AbortSignal) => Promise<unknown>;

interface Runner {
    /** Прочитать сейчас; идёт чтение — `queue` просит ещё одно после него, без него — пропустить. */
    run: (queue: boolean) => void;
    /** Прочитать, но не раньше `settle` мс от прошлого начала. */
    soon: () => void;
    stop: () => void;
}

function runner(read: Read, settle: number): Runner {
    const abort = new AbortController();
    let busy = false;
    let again = false;
    let startedAt = 0;
    let timer = 0;
    const run = (queue: boolean): void => {
        if (abort.signal.aborted) return;
        if (busy) {
            again ||= queue;
            return;
        }
        busy = true;
        again = false;
        startedAt = Date.now();
        void read(abort.signal)
            .catch(() => undefined)
            .finally(() => {
                busy = false;
                if (again) run(true);
            });
    };
    return {
        run,
        soon: () => {
            if (timer || abort.signal.aborted) return;
            const wait = startedAt + settle - Date.now();
            if (wait <= 0) return run(true);
            timer = window.setTimeout(() => {
                timer = 0;
                run(true);
            }, wait);
        },
        stop: () => {
            abort.abort();
            window.clearTimeout(timer);
        },
    };
}

export interface Poll {
    read: Read;
    /** Выключен — не читать вовсе. */
    on?: boolean;
    /** Что читается (колонка, устройство): сменилось — идущее обрывается, читается сразу. */
    deps?: unknown[];
    /** Раз в столько мс; 0 — только по переменам. */
    every?: number;
    /** Номер перемены (panel.version): вырос — перечитать, не чаще раза в `settle`. */
    version?: number;
    settle?: number;
}

/** Читать сразу, раз в `every` и по переменам — без наложений. Возвращает «прочитать сейчас» (после своего действия). */
export function usePoll({ read, on = true, deps = [], every = 0, version = 0, settle = SETTLE_MS }: Poll): () => void {
    const latest = useRef(read);
    latest.current = read;
    const control = useRef<Runner | null>(null);
    useEffect(() => {
        if (!on) return;
        const poll = runner((signal) => latest.current(signal), settle);
        control.current = poll;
        poll.run(true);
        return () => {
            poll.stop();
            if (control.current === poll) control.current = null;
        };
    }, [on, ...deps]);
    useEffect(() => {
        if (!on || !every) return;
        const id = window.setInterval(() => control.current?.run(false), every);
        return () => window.clearInterval(id);
    }, [on, every, ...deps]);
    const seen = useRef(version);
    useEffect(() => {
        if (seen.current === version) return;
        seen.current = version;
        control.current?.soon();
    }, [version]);
    return () => control.current?.run(true);
}
