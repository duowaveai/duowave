// Что пульт делает по вестям мастерской (docs/plans/voice-creation.md): агент закончил — полоса «Готово…» с
// подсказками (просили голосом в этом пульте — он 6 секунд слушает ответ без нажатия); колонка попросила открыть
// «Примерку» или «да» — окно открывается. Рисовать нечего: только слушает вести.

import { useEffect, useRef } from 'preact/hooks';

import { api, can } from '../api';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { announceReady, failed } from './creation';
import { onWorkshopChange } from './drafts';
import { askedByVoice, listenFor } from './mic';
import { showStrip } from './strip';

/** Сколько пульт слушает ответ на «Готово», если просили голосом в нём. */
export const ANSWER_SECONDS = 6;

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
            if (change !== 'ready' && change !== 'failed') return;
            void api
                .workshopDraft(draft)
                .then((view) => {
                    if (view.meta.stage === 'failed') {
                        showStrip({ kind: 'note', head: 'Не вышло', text: view.meta.note ?? `«${view.title ?? draft}»: что случилось — в мастерской.` }, 10_000);
                        return;
                    }
                    if (view.meta.stage !== 'ready') return;
                    const voice = askedByVoice(draft);
                    announceReady(
                        view,
                        {
                            fit: () => go('fitting', draft),
                            desk: () => go('fitting', draft, undefined, { yes: '' }),
                            redo: () => go('fitting', draft, undefined, { redo: '' }),
                            park: () => void api.parkDraft(draft).catch((e) => showStrip({ kind: 'note', head: 'Не вышло', text: failed(e) }, 6000)),
                            drop: () =>
                                void ask({ title: 'Удалить черновик?', text: `«${view.title ?? draft}» уйдёт из мастерской.`, ok: 'Удалить', danger: true }).then(
                                    (yes) => yes && api.deleteDraft(draft),
                                ),
                        },
                        voice ? ANSWER_SECONDS : 0,
                    );
                    if (voice) listenFor(ANSWER_SECONDS * 1000);
                })
                .catch(() => undefined);
        });
    }, []);
    return null;
}
