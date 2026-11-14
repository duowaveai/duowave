// «Ждёт вас» (docs/plans/audio-interface-2.md, «К3»): работы, чей вопрос ждёт человека — «На стол или примерить?»,
// «Продолжить?» — спрошен, а ответа не было (промолчали) или ещё не спрошен. Рядом со «Сказать» — пилюля с числом;
// нажатие показывает на полосе первую из них с кнопками ответа, следующее — следующую. Нет таких — пилюли нет.
// Список шлюза (`GET /api/v1/works?waiting=1`) перечитывается на каждую перемену работ.

import { useEffect, useRef, useState } from 'preact/hooks';

import { WorkView, api, can } from '../api';
import { tr } from '../i18n';
import { onWorkChange } from './works';

type Shower = (work: WorkView) => void;

let shower: Shower | null = null;

/** Кто показывает работу на полосе с кнопками ответа (voice/flow.tsx). */
export function setWorkShower(show: Shower | null): void {
    shower = show;
}

/** Работы, чей вопрос ждёт человека; перечитываются на перемены работ. */
function useWaiting(): WorkView[] {
    const [works, setWorks] = useState<WorkView[]>([]);
    useEffect(() => {
        if (!can('plugins.manage')) return;
        let alive = true;
        const load = () =>
            void api
                .waitingWorks()
                .then((list) => alive && setWorks(list))
                .catch(() => undefined);
        load();
        const stop = onWorkChange(load);
        return () => {
            alive = false;
            stop();
        };
    }, []);
    return works;
}

/** Пилюля «Ждёт вас · N» рядом со «Сказать». */
export function WaitingPill({ place }: { place: 'dock' | 'side' }) {
    const works = useWaiting();
    const next = useRef(0);
    if (!works.length) return null;
    const show = () => {
        const work = works[next.current % works.length];
        next.current += 1;
        shower?.(work);
    };
    return (
        <button type="button" class={`waiting-pill waiting-pill-${place}`} title={tr.voice.waitingTitle} onClick={show}>
            {tr.voice.waitingPill(works.length)}
        </button>
    );
}
