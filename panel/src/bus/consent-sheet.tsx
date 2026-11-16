// «Да», как в телефоне (docs/plans/bus.md, решения 2 и 10): приложение впервые тянется к тому, что попросило в
// манифесте, — лист снизу на любом экране: «Покупки» хочет звать «Заметки». «Разрешить» запоминается связью (поменять —
// в окне приложения, «Связи»), «Не разрешать» — тоже: приложение больше не спрашивает. Опасное действие, которое начал
// не человек, спрашивает каждый раз и разрешается один раз. Отвечает администратор дома: у остальных листа нет.

import { useEffect, useState } from 'preact/hooks';

import { BusQuestion, api, can, problemOf } from '../api';
import { tr } from '../i18n';
import { Sheet } from '../ui/sheet';
import { busChanged, useBusVersion } from './state';
import { DANGER_TITLES } from './words';

/** Вопросы, отложенные «Позже» в этой вкладке: вернутся после перезагрузки или в окне приложения. */
const later = new Set<string>();

export function ConsentSheet() {
    const version = useBusVersion();
    const [questions, setQuestions] = useState<BusQuestion[]>([]);
    const [busy, setBusy] = useState<'yes' | 'no' | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [, setHidden] = useState(0);
    const allowed = can('plugins.manage');
    useEffect(() => {
        if (!allowed) return;
        let alive = true;
        api.busQuestions()
            .then((answer) => alive && setQuestions(answer.questions))
            .catch(() => alive && setQuestions([]));
        return () => {
            alive = false;
        };
    }, [version, allowed]);
    // Старший вопрос — первым: так они и пришли.
    const question = questions.filter((entry) => !later.has(entry.id)).sort((a, b) => a.at.localeCompare(b.at))[0];
    if (!allowed || !question) return null;
    const once = question.kind === 'once';
    const answer = async (yes: boolean): Promise<void> => {
        setBusy(yes ? 'yes' : 'no');
        setError(null);
        try {
            await api.busAnswer(question.id, yes);
            setQuestions((all) => all.filter((entry) => entry.id !== question.id));
            busChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const postpone = (): void => {
        later.add(question.id);
        setHidden((n) => n + 1);
    };
    const waiting = questions.filter((entry) => !later.has(entry.id)).length;
    const spin = (what: 'yes' | 'no') => busy === what && <span class="spinner spinner-sm" aria-hidden="true" />;
    return (
        <Sheet
            title={once ? tr.bus.wantsDanger(question.fromTitle) : tr.bus.asksAccess(question.fromTitle)}
            meta={waiting > 1 ? <span class="win-chip">{tr.bus.moreQuestions(waiting - 1)}</span> : undefined}
            onClose={postpone}
            footer={
                <>
                    <button class="btn btn-quiet" disabled={!!busy} onClick={postpone}>
                        {tr.bus.later}
                    </button>
                    <button class="btn btn-plain" disabled={!!busy} aria-busy={busy === 'no' || undefined} onClick={() => void answer(false)}>
                        {spin('no')}
                        {once ? tr.bus.no : tr.bus.deny}
                    </button>
                    <button class="btn btn-main" disabled={!!busy} aria-busy={busy === 'yes' || undefined} onClick={() => void answer(true)}>
                        {spin('yes')}
                        {once ? tr.bus.allowOnce : tr.bus.allow}
                    </button>
                </>
            }
        >
            <p class="win-lead">{tr.bus.wants(question.fromTitle, question.what)}</p>
            {once && question.danger && <p class="win-callout m-0">{tr.bus.dangerNote(DANGER_TITLES[question.danger] ?? question.danger)}</p>}
            {question.root && <p class="win-note">{tr.bus.startedBy(question.root)}</p>}
            <p class="win-note">{once ? tr.bus.answerOnce : tr.bus.answerRemembered}</p>
            {error && (
                <p class="win-callout m-0" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
