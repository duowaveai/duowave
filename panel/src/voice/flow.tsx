// Что пульт делает по вестям мастерской и работ (docs/plans/voice-creation.md; docs/plans/audio-interface.md,
// «Пульт»): работа кончилась — полоса со строкой итога шлюза и подсказками; работу этого компьютера в режиме «Микрофон»
// пульт ещё и говорит своим голосом, договорил — сообщает шлюзу и 6 секунд слушает ответ без нажатия (ответ идёт слою:
// «да» ставит на стол). Колонка попросила открыть «Примерку» или «да» — окно открывается; приложению на столе не хватает
// ключа (`open:needs`) — открывается его окно с полями. Рисовать нечего: только слушает вести.

import { useEffect, useRef } from 'preact/hooks';

import { WorkView, api, can } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { announceReady, failed } from './creation';
import { onWorkshopChange } from './drafts';
import { listenFor, micOn, onMic } from './mic';
import { answering, onAnswering, sayAloud } from './speak';
import { showStrip } from './strip';
import { setWorkShower } from './waiting';
import { SAID_ON, askedHere, onWorkChange, readWork, speaksHere, workHead } from './works';

/** Сколько пульт слушает ответ на «Готово», если работа — его. */
export const ANSWER_SECONDS = 6;
/** «Готово» ждёт паузы не дольше: потом остаётся на полосе и в «Ждёт вас». */
const IDLE_WAIT_MS = 120_000;

/**
 * Пульт не слушает и не говорит (docs/plans/audio-interface-2.md, «К4»: не перебивать): true — пауза настала, false —
 * не дождались за `ms`.
 */
function idle(ms: number): Promise<boolean> {
    const quiet = () => !micOn() && !answering();
    if (quiet()) return Promise.resolve(true);
    return new Promise((resolve) => {
        const stops: Array<() => void> = [];
        const done = (value: boolean) => {
            stops.forEach((stop) => stop());
            window.clearTimeout(timer);
            resolve(value);
        };
        const check = () => quiet() && done(true);
        const timer = window.setTimeout(() => done(false), ms);
        stops.push(onMic(check), onAnswering(check));
    });
}

/** Строки «Готово» звучат по одной: вторая ждёт, пока договорится первая. */
let saying: Promise<void> = Promise.resolve();

export function CreationFlow() {
    const panel = usePanel();
    const latest = useRef(panel);
    latest.current = panel;
    useEffect(() => {
        if (!can('plugins.manage')) return;
        return onWorkshopChange((draft, change) => {
            if (!draft) return;
            const go = latest.current.go;
            if (change === 'open:fitting') return go('fitting', draft);
            if (change === 'open:consent') return go('fitting', draft, undefined, { yes: '' });
            // На столе, но не хватает ключа: окно приложения с его полями.
            if (change === 'open:needs') return go('skills', 'installed', draft);
        });
    }, []);
    useEffect(() => {
        if (!can('plugins.manage')) return;
        return onWorkChange((id, change) => {
            if (!id || !SAID_ON.includes(change)) return;
            void readWork(id).then((work) => work && announce(work));
        });
    }, []);
    // «Ждёт вас»: работа показывается на полосе с кнопками ответа, без голоса.
    useEffect(() => {
        setWorkShower((work) => void announce(work, true));
        return () => setWorkShower(null);
    }, []);
    /**
     * Строка итога работы на полосе — и голосом, когда это работа этого компьютера в режиме «Микрофон». `quiet` — только
     * полоса («Ждёт вас»).
     */
    const announce = async (work: WorkView, quiet = false): Promise<void> => {
        const line = work.result?.line;
        if (!line) return;
        const go = latest.current.go;
        const draft = work.target;
        if (work.kind === 'tool' || work.kind === 'task') {
            // Фоновое дело (docs/plans/long-tools.md): строка «Готово» и кнопка с пересказом и тем, что пришло.
            const { summary, data } = work.result ?? { line };
            const shown = [summary && `${tr.voice.resultRetold}: ${summary}`, data && `${tr.voice.resultData}:\n${data.slice(0, 4000)}`]
                .filter(Boolean)
                .join('\n\n');
            const chips = shown
                ? [{ label: tr.voice.resultChip, act: () => void ask({ title: work.title, text: shown, ok: tr.voice.resultClose }) }]
                : undefined;
            showStrip({ kind: work.state === 'failed' ? 'note' : 'done', head: workHead(work), text: line, chips }, 20_000);
        } else if (work.state === 'failed' || work.state === 'done' || !draft) {
            showStrip({ kind: work.state === 'done' ? 'done' : 'note', head: workHead(work), text: line }, 10_000);
        } else {
            const view = await api.workshopDraft(draft).catch(() => null);
            if (!view) return;
            const voice = !quiet && speaksHere(work) && askedHere(work) && !!work.question;
            announceReady(
                view,
                {
                    fit: () => go('fitting', draft),
                    desk: () => go('fitting', draft, undefined, { yes: '' }),
                    redo: () => go('fitting', draft, undefined, { redo: '' }),
                    park: () => void api.parkDraft(draft).catch((e) => showStrip({ kind: 'note', head: tr.voice.failedHead, text: failed(e) }, 6000)),
                    drop: () =>
                        void ask({ title: tr.voice.deleteDraftTitle, text: tr.voice.deleteDraftText(view.title ?? draft), ok: tr.voice.delete, danger: true }).then(
                            (yes) => yes && api.deleteDraft(draft),
                        ),
                },
                voice ? ANSWER_SECONDS : 0,
                line,
                workHead(work),
            );
        }
        if (quiet || !speaksHere(work) || !askedHere(work)) return;
        // Компьютер говорит «Готово…» сам, но не перебивает: ждёт, пока пульт не слушает и не говорит. Договорил —
        // шлюз открывает вопрос, пульт слушает ответ.
        const turn = saying.then(async () => {
            if (!(await idle(IDLE_WAIT_MS))) return;
            if (!(await sayAloud(line))) return;
            const said = await api.workSaid(work.id).catch(() => null);
            if (said?.question) listenFor(ANSWER_SECONDS * 1000, 'ready');
            // Микрофон слушает ответ: следующее «Готово» подождёт и его.
            await idle(IDLE_WAIT_MS);
        });
        saying = turn.catch(() => undefined);
        await turn;
    };
    return null;
}
