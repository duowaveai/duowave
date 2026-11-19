// «Ждёт вас» (docs/plans/audio-interface-2.md, «К3»): работы, чей вопрос ждёт человека — «На стол или примерить?»,
// «Продолжить?» — спрошен, а ответа не было (промолчали) или ещё не спрошен. Рядом со «Сказать» — пилюля с числом;
// нажатие показывает первую из них, следующее — следующую: «Готово» — на полосе с кнопками ответа, остальное (и то, что
// полоса показать не смогла) — страница работы с её кнопками (voice/waiting-model.ts). Нет таких — пилюли нет.
// Список шлюза (`GET /api/v1/works?waiting=1`) перечитывается на каждую перемену работ и на каждое нажатие: число и то,
// что за ним, — одно и то же.

import { useEffect, useRef, useState } from 'preact/hooks';

import { WorkView, api, can } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { waitPage, waitShownOn } from './waiting-model';
import { onWorkChange } from './works';

/** Показывает работу на полосе с кнопками ответа; false — показать не вышло. */
type Shower = (work: WorkView) => Promise<boolean>;

let shower: Shower | null = null;

/** Кто показывает работу на полосе с кнопками ответа (voice/flow.tsx). */
export function setWorkShower(show: Shower | null): void {
    shower = show;
}

/** Работы, чей вопрос ждёт человека, и как перечитать их сейчас; перечитываются и на перемены работ. */
function useWaiting(): [WorkView[], () => Promise<WorkView[]>] {
    const [works, setWorks] = useState<WorkView[]>([]);
    const alive = useRef(true);
    const reload = () =>
        api
            .waitingWorks()
            .then((list) => {
                if (alive.current) setWorks(list);
                return list;
            })
            .catch(() => works);
    useEffect(() => {
        alive.current = true;
        if (!can('plugins.manage')) return;
        void reload();
        const stop = onWorkChange(() => void reload());
        return () => {
            alive.current = false;
            stop();
        };
    }, []);
    return [works, reload];
}

/** Пилюля «Ждёт вас · N» рядом со «Сказать». */
export function WaitingPill({ place }: { place: 'dock' | 'side' }) {
    const panel = usePanel();
    const [works, reload] = useWaiting();
    const next = useRef(0);
    if (!works.length) return null;
    const show = async () => {
        // Свежий список: работа могла уйти (на неё ответили, вещь поставили) — число и то, что за ним, сходятся.
        const list = await reload();
        if (!list.length) return;
        const work = list[next.current % list.length];
        next.current += 1;
        if (waitShownOn(work) === 'strip' && shower && (await shower(work))) return;
        const page = waitPage(work, can('settings.edit'));
        if (page) panel.go(page.section, page.tab, page.item);
    };
    return (
        <button type="button" class={`waiting-pill waiting-pill-${place}`} title={tr.voice.waitingTitle} onClick={() => void show()}>
            {tr.voice.waitingPill(works.length)}
        </button>
    );
}
