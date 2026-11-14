// Долгие задачи приложений в пульте (docs/plans/feed.md, этап 1; шлюз — gateway/src/tasks). Приложение начинает задачу
// и сообщает ход; шлюз говорит о каждой перемене по живому каналу (`tasks.changed`, чья — id приложения, не чаще двух
// раз в секунду на задачу), а пульт перечитывает задачи этого приложения: `GET /api/v1/tasks?app=<id>`. Читают только
// те приложения, чьи окна сейчас следят (мост окна — frame/host.tsx, `useTask` в SDK окна). Чтения одного приложения не
// накладываются: перемена во время чтения — ещё одно чтение после него. Пока задача идёт, а канал молчит дольше трёх
// секунд, пульт читает сам.

import { AppTask, api } from '../api';
import { tr } from '../i18n';

interface Watch {
    tasks: AppTask[];
    /** Задачи прочитаны хоть раз. */
    ready: boolean;
    listeners: Set<() => void>;
    reading: boolean;
    again: boolean;
    /** Запасное чтение, пока задача идёт: канал мог замолчать (сеть, проход), а ход — нет. */
    timer: number | null;
}

/** Задача идёт, а канал молчит столько — прочитать самим. */
const QUIET_MS = 3000;

const watched = new Map<string, Watch>();

async function read(app: string): Promise<void> {
    const watch = watched.get(app);
    if (!watch) return;
    if (watch.reading) {
        watch.again = true;
        return;
    }
    watch.reading = true;
    if (watch.timer !== null) window.clearTimeout(watch.timer);
    watch.timer = null;
    try {
        const { tasks } = await api.tasks(app);
        watch.tasks = tasks;
        watch.ready = true;
        for (const listener of [...watch.listeners]) listener();
    } catch {
        // Шлюз не ответил: что знали, то и показываем; следующая перемена или новый hello перечитает.
    } finally {
        watch.reading = false;
        if (watch.again) {
            watch.again = false;
            void read(app);
        } else if (watched.get(app) === watch && watch.tasks.some((task) => task.state === 'running')) {
            watch.timer = window.setTimeout(() => void read(app), QUIET_MS);
        }
    }
}

/** Перемена, о которой сказал живой канал: задачи приложения `subject` (или всех — канал открылся заново). */
export function noteTasksChange(name: string, subject?: string): void {
    if (name === 'hello') for (const app of [...watched.keys()]) void read(app);
    else if (name === 'tasks.changed') {
        if (subject) void read(subject);
        else for (const app of [...watched.keys()]) void read(app);
    }
}

/** Задачи приложения, как пульт их знает сейчас: что идёт — первым, потом новые. */
export function tasksOf(app: string): { tasks: AppTask[]; ready: boolean } {
    const watch = watched.get(app);
    return { tasks: watch?.tasks ?? [], ready: watch?.ready ?? false };
}

/** Следить за задачами приложения: `listener` — на каждое прочтение. Возвращает, как перестать. */
export function watchTasks(app: string, listener: () => void): () => void {
    let watch = watched.get(app);
    if (!watch) {
        watch = { tasks: [], ready: false, listeners: new Set(), reading: false, again: false, timer: null };
        watched.set(app, watch);
    }
    watch.listeners.add(listener);
    void read(app);
    return () => {
        const now = watched.get(app);
        now?.listeners.delete(listener);
        if (now && now.listeners.size === 0) {
            if (now.timer !== null) window.clearTimeout(now.timer);
            watched.delete(app);
        }
    };
}

/** «Отменить»: шлюз просит приложение остановиться, задачу заканчивает оно само. Только своя задача приложения `app`. */
export async function cancelTask(app: string, id: string): Promise<void> {
    if (!tasksOf(app).tasks.some((task) => task.id === id)) throw new Error(tr.core.notThisAppTask);
    await api.cancelTask(id);
    void read(app);
}
