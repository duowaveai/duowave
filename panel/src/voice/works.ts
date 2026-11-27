// Работы разговора в пульте (docs/plans/audio-interface.md, «Пульт: фраза идёт в разговор»): шлюз говорит о каждой
// перемене работы событием `work.changed` — её id и вид перемены (`started`, `note`, `ready`, `unfinished`, `failed`,
// `done`, `said`…). Работа кончилась — пульт пишет её строку итога на полосе («Часы: готово. На стол или примерить?»),
// а работу этого компьютера в режиме «Микрофон» ещё и говорит своим голосом: договорил — сообщает шлюзу
// (`POST /api/v1/works/{id}/said`: открывается вопрос) и 6 секунд слушает ответ без нажатия; ответ идёт слою как есть.
// С колонкой «Готово» своей работы говорит колонка.

import { WorkView, api, can } from '../api';
import { tr } from '../i18n';
import { onComputer } from '../voice-input';

type Listener = (work: string, change: string) => void;

const listeners = new Set<Listener>();
/** Работы, начатые или дополненные из этого пульта: их «Готово» пульт говорит и слушает ответ. */
const here = new Set<string>();

/** Перемена работы с шлюза (живой канал). Без работы — перечитать всё. */
export function noteWorkChange(work?: string, change?: string): void {
    for (const listener of [...listeners]) listener(work ?? '', change ?? '');
}

/** Слушать перемены работ. Возвращает, как перестать. */
export function onWorkChange(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Фраза этого пульта начала или дополнила работу (`work` в ответе разговора). */
export function startedHere(work: string): void {
    here.add(work);
}

/** Чем кончилась работа так, что о ней говорят: готово, не доделано, не вышло, правка уже на столе. */
export const SAID_ON: string[] = ['ready', 'unfinished', 'failed', 'done'];

/**
 * Где работу трогали последней (docs/plans/audio-interface-3.md, «Н1»): откуда пришла её последняя фраза — начало,
 * заметка, «и ещё…»; там и звучит её «Готово».
 */
export function touchedOn(work: WorkView): string {
    let device = work.origin.device;
    let at = work.startedAt;
    for (const note of work.notes) {
        if (note.at >= at) {
            at = note.at;
            device = note.origin.device;
        }
    }
    return device;
}

/** Строку итога работы этого компьютера говорит пульт: в режиме «Микрофон», работу последней трогали здесь. */
export function speaksHere(work: WorkView): boolean {
    return onComputer() && touchedOn(work) === 'computer' && !!work.result?.line;
}

/** Работа этого пульта: её ждут здесь — после «Готово» слушать ответ. */
export function askedHere(work: WorkView): boolean {
    return here.has(work.id) || touchedOn(work) === 'computer';
}

/** Прочитать работу; нет права — нет и работы. */
export async function readWork(id: string): Promise<WorkView | null> {
    if (!can('plugins.manage') && !can('station.view')) return null;
    return api.work(id).catch(() => null);
}

/** Голова полосы по тому, чем кончилась работа. */
export function workHead(work: WorkView): string {
    if (work.state === 'failed') return tr.voice.failedHead;
    if (work.state === 'unfinished') return tr.voice.unfinishedHead;
    if (work.state === 'done') return tr.voice.done;
    return tr.voice.readyHead;
}
