// «Да», как в телефоне (docs/plans/bus.md, решения 2 и 10): приложение впервые тянется к тому, что попросило в
// манифесте, — лист снизу на любом экране: «Покупки» хочет звать «Заметки». «Разрешить» запоминается связью (поменять —
// в окне приложения, «Связи»), «Не разрешать» — тоже: приложение больше не спрашивает. Опасное действие, которое начал
// не человек, спрашивает каждый раз и разрешается один раз. Отвечает администратор дома: у остальных листа нет.

import { useEffect, useState } from 'preact/hooks';

import { BusQuestion, api, can, problemOf } from '../api';
import { Action } from '../ui/cards';
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
    return (
        <Sheet
            title={once ? `«${question.fromTitle}» хочет сделать опасное` : `«${question.fromTitle}» просит доступ`}
            about={waiting > 1 ? `ещё вопросов: ${waiting - 1}` : undefined}
            onClose={postpone}
            footer={
                <div class="flex flex-wrap items-center gap-8">
                    <Action tone="on" label={once ? 'Разрешить один раз' : 'Разрешить'} busy={busy === 'yes'} disabled={!!busy} onClick={() => void answer(true)} />
                    <Action label={once ? 'Нет' : 'Не разрешать'} busy={busy === 'no'} disabled={!!busy} onClick={() => void answer(false)} />
                    <Action label="Позже" disabled={!!busy} onClick={postpone} />
                </div>
            }
        >
            <p class="t-body m-0">
                «{question.fromTitle}» хочет {question.what}.
            </p>
            {once && question.danger && (
                <p class="t-caption m-0 c-pill-yandex-fg">
                    Опасное: {DANGER_TITLES[question.danger] ?? question.danger}. Этого не начинал человек — разрешите, только если
                    ждёте этого сейчас.
                </p>
            )}
            {question.root && <p class="t-caption m-0">Начал: {question.root}.</p>}
            <p class="t-caption m-0">
                {once
                    ? 'Ответ — на это одно действие: в следующий раз приложение спросит снова.'
                    : 'Ответ запомнится, как в телефоне: приложение больше не спросит. Поменять — в окне приложения, «Связи».'}
            </p>
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
