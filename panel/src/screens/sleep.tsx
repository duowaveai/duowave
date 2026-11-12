// «За ночь» и «Сон и рефлексия» (docs/plans/memory.md, «Сон и рефлексия»; макет — вкладка «За ночь» в дизайне памяти).
// Сон — ночная работа шлюза по частям; первая и пока единственная часть — рефлексия: ночью она обдумывает разговоры,
// наводит порядок в записях и пишет итог дня и дневник персонажа. Любую её перемену можно отменить, всю ночь — откатить.
// Рефлексия выключена по умолчанию (решение владельца 12.11). Смотреть — memory.view, менять — memory.manage.

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';

import {
    api,
    can,
    plural,
    problemOf,
    MemoryChange,
    MemoryModel,
    ReflectionView,
    SleepNote,
    SleepPartReport,
    SleepRun,
    SleepRunDetail,
    SleepView,
} from '../api';
import {
    IconChevronLeft,
    IconClose,
    IconHome,
    IconJournal,
    IconLinks,
    IconMemory,
    IconMoon,
    IconPage,
    IconSparkle,
    IconTimer,
    IconUndo,
    IconUser,
} from '../icons';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { Switch } from '../ui/switch';

const MONTHS = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

/** «2026-11-13» → «13 ноября». */
function longDay(day: string): string {
    const [, month, date] = day.split('-').map(Number);
    return month && date ? `${date} ${MONTHS[month - 1]}` : day;
}

/** «2026-11-13» → «13.11». */
function shortDay(day: string): string {
    const [, month, date] = day.split('-');
    return month && date ? `${date}.${month}` : day;
}

/** «4:07» по часам этого браузера. */
function clock(iso: string): string {
    const at = new Date(iso);
    return `${at.getHours()}:${String(at.getMinutes()).padStart(2, '0')}`;
}

function nightTitle(run: SleepRun): string {
    return run.trigger === 'manual' ? `Сон по кнопке · ${shortDay(run.night)}, ${clock(run.startedAt)}` : `Ночь на ${shortDay(run.night)}`;
}

/** Что ночь сделала — одной строкой для списка. */
function nightLine(run: SleepRun): string {
    if (run.state === 'running') return 'Идёт…';
    if (run.rolledBackAt) return 'Откачена: всё, что она сделала, вернулось.';
    const part = run.parts.find((report) => report.state !== 'nothing') ?? run.parts[0];
    return part ? part.summary : 'Ничего не делала.';
}

const STATES: Record<SleepPartReport['state'], string> = {
    done: 'сделано',
    nothing: 'без работы',
    waiting: 'ждёт',
    failed: 'сбой',
};

/** Перемена «Порядка» — значком и словами. */
function orderRow(change: MemoryChange): { icon: JSX.Element; title: string } {
    const said = `«${change.summary}»`;
    switch (change.action) {
        case 'merge':
            return { icon: <IconLinks size={14} />, title: `Склеен повтор: ${said}` };
        case 'close':
            return { icon: <IconClose size={14} />, title: `Больше не так: ${said}` };
        case 'past':
            return { icon: <IconJournal size={14} />, title: `Ушло в историю: ${said}` };
        case 'date':
            return { icon: <IconTimer size={14} />, title: `Дата вместо «завтра»: ${said}` };
        case 'alias':
            return { icon: <IconUser size={14} />, title: `Другое имя: ${change.summary}` };
        case 'importance':
            return { icon: <IconSparkle size={14} />, title: `Важность пересчитана: ${said}` };
        default:
            return { icon: <IconPage size={14} />, title: `Переписано: ${said}` };
    }
}

function noteTitle(note: SleepNote): string {
    return note.characterId ? `Дневник: ${note.characterTitle ?? note.characterId} · ${shortDay(note.day)}` : `Итог дня для дома · ${shortDay(note.day)}`;
}

// ───────── «За ночь» ─────────

/** «Приложения» → «Память» → «За ночь»: список ночей, по нажатию — отчёт ночи. */
export function NightTab({
    night,
    onOpen,
    onBack,
    onSettings,
}: {
    night: string | null;
    onOpen: (id: string) => void;
    onBack: () => void;
    onSettings: () => void;
}) {
    if (night) return <NightView id={night} onBack={onBack} />;
    return <NightList onOpen={onOpen} onSettings={onSettings} />;
}

/** Память ждёт модель: включена, разговоры есть, а думать нечем — ссылка на выбор модели. */
function waitsForModel(reflection: ReflectionView | null): boolean {
    return !!reflection && reflection.enabled && reflection.pending > 0 && !!reflection.waiting;
}

function NightList({ onOpen, onSettings }: { onOpen: (id: string) => void; onSettings: () => void }) {
    const panel = usePanel();
    const manage = can('memory.manage');
    const [runs, setRuns] = useState<SleepRun[] | null>(null);
    const [sleep, setSleep] = useState<SleepView | null>(null);
    const [reflection, setReflection] = useState<ReflectionView | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const reread = usePoll({
        every: 20_000,
        version: panel.version,
        read: (signal) =>
            Promise.all([api.sleepRuns(30, signal), api.sleep(signal), api.reflection(signal)])
                .then(([list, view, mind]) => {
                    if (signal.aborted) return;
                    setRuns(list);
                    setSleep(view);
                    setReflection(mind);
                })
                .catch((e) => {
                    if (!signal.aborted) setProblem(problemOf(e));
                }),
    });
    const act = (job: () => Promise<unknown>) => {
        setBusy(true);
        setProblem(null);
        job()
            .then(reread)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const running = sleep?.running ?? null;
    return (
        <>
            {reflection && !reflection.enabled && (
                <section class="card glass card-list" aria-labelledby="rc-off-h">
                    <div class="rc-card-head">
                        <span class="rc-night-mi" aria-hidden="true">
                            <IconMoon size={18} />
                        </span>
                        <div class="flex-1 min-w-0 flex flex-col">
                            <h2 id="rc-off-h" class="t-title-xs m-0">
                                Рефлексия выключена
                            </h2>
                            <span class="w-cap">
                                Включите — и ночью память обдумает разговоры дня: склеит повторы, закроет устаревшее, напишет итог дня. Утром здесь
                                будет отчёт, любую перемену можно отменить.
                            </span>
                        </div>
                        {manage && (
                            <button class="w-btn" disabled={busy} onClick={() => act(() => api.setReflection({ enabled: true }))}>
                                Включить
                            </button>
                        )}
                    </div>
                </section>
            )}
            <section class="card glass card-list" aria-labelledby="rc-nights-h">
                <div class="section-head">
                    <h2 id="rc-nights-h" class="t-title-xs m-0">
                        Ночи
                    </h2>
                    {manage && (
                        <button
                            class="w-btn rc-icon-btn"
                            disabled={busy || !!running || !!sleep?.whyNot}
                            onClick={() => act(() => api.runSleep().then(({ id }) => onOpen(id)))}
                        >
                            <IconMoon size={14} />
                            {running ? 'Сон идёт…' : 'Запустить сон сейчас'}
                        </button>
                    )}
                </div>
                {sleep && (
                    <p class="w-cap m-0">
                        Сон — в {sleep.time} по часам колонки, если у включённой части есть работа.{' '}
                        {running
                            ? 'Сон идёт сейчас.'
                            : (sleep.whyNot ??
                              (reflection && reflection.pending > 0
                                  ? `Ждут рефлексии: ${reflection.pending} ${plural(reflection.pending, 'разговор', 'разговора', 'разговоров')}.`
                                  : ''))}
                        {waitsForModel(reflection) && (
                            <>
                                {' '}
                                <button class="rc-link" onClick={onSettings}>
                                    Выбрать модель памяти
                                </button>
                            </>
                        )}
                    </p>
                )}
                {problem && (
                    <p class="w-cap c-dot-offline m-0" role="alert">
                        {problem}
                    </p>
                )}
                {!runs ? (
                    <p class="w-cap m-0">Читаю…</p>
                ) : runs.length === 0 ? (
                    <p class="w-cap m-0">Ночей ещё не было. Ночь без разговоров проходит без работы.</p>
                ) : (
                    <ul class="rc-nights">
                        {runs.map((run) => (
                            <li key={run.id}>
                                <button class={run.state === 'nothing' || run.rolledBackAt ? 'rc-night rc-night-dim' : 'rc-night'} onClick={() => onOpen(run.id)}>
                                    <span class="rc-night-mi" aria-hidden="true">
                                        <IconMoon size={15} />
                                    </span>
                                    <span class="min-w-0 flex flex-col">
                                        <span class="w-strong">{nightTitle(run)}</span>
                                        <span class="w-cap rc-ellipsis">{nightLine(run)}</span>
                                    </span>
                                    {run.state === 'partial' && !run.rolledBackAt && <span class="rc-state rc-state-waiting">не закончена</span>}
                                </button>
                            </li>
                        ))}
                    </ul>
                )}
            </section>
        </>
    );
}

function NightView({ id, onBack }: { id: string; onBack: () => void }) {
    const panel = usePanel();
    const manage = can('memory.manage');
    const [run, setRun] = useState<SleepRunDetail | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const reread = usePoll({
        every: 5_000,
        version: panel.version,
        deps: [id],
        read: (signal) =>
            api
                .sleepRun(id, signal)
                .then((fresh) => {
                    if (signal.aborted) return;
                    setRun(fresh);
                    setProblem(null);
                })
                .catch((e) => {
                    if (!signal.aborted) setProblem(problemOf(e));
                }),
    });
    const act = (job: () => Promise<unknown>) => {
        setBusy(true);
        setProblem(null);
        job()
            .then(reread)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const back = (
        <button class="deeper-back" onClick={onBack}>
            <IconChevronLeft size={16} />
            Все ночи
        </button>
    );
    if (!run) {
        return (
            <>
                {back}
                <p class={problem ? 'w-cap c-dot-offline' : 'w-cap'} role={problem ? 'alert' : undefined}>
                    {problem ?? 'Читаю…'}
                </p>
            </>
        );
    }
    const night = run.details.reflection;
    const changes = night?.changes ?? [];
    const order = changes.filter((change) => change.target === 'fact' || change.target === 'entity');
    const notes = night?.notes ?? [];
    const noteChange = (note: SleepNote) => changes.find((change) => change.target === 'note' && change.targetId === note.id && !change.undoneAt);
    const goneNotes = changes.filter((change) => change.target === 'note' && change.undoneAt && !notes.some((note) => note.id === change.targetId));
    const live = changes.some((change) => !change.undoneAt);
    const rollBack = async () => {
        const sure = await ask({
            title: 'Откатить ночь?',
            text: 'Всё, что рефлексия сделала этой ночью, вернётся как было, а её разговоры снова будут ждать рефлексии.',
            ok: 'Откатить',
        });
        if (sure) act(() => api.rollBackSleepRun(run.id));
    };
    const undo = (change: MemoryChange) => act(() => api.undoMemoryChange(change.id));
    const undoButton = (change: MemoryChange | undefined) =>
        !change ? (
            <span />
        ) : change.undoneAt ? (
            <span class="w-cap">отменено</span>
        ) : manage && !run.rolledBackAt ? (
            <button class="w-btn rc-undo" disabled={busy} onClick={() => undo(change)}>
                <IconUndo size={13} />
                Отменить
            </button>
        ) : (
            <span />
        );
    const span = run.finishedAt ? `${clock(run.startedAt)}–${clock(run.finishedAt)}` : `с ${clock(run.startedAt)}`;
    return (
        <>
            {back}
            <section class="card glass card-list" aria-labelledby="rc-night-h">
                <div class="rc-card-head rc-night-head">
                    <span class="rc-night-mi" aria-hidden="true">
                        <IconMoon size={18} />
                    </span>
                    <div class="flex-1 flex flex-col rc-night-title">
                        <h2 id="rc-night-h" class="t-title-xs m-0">
                            {run.trigger === 'manual' ? `Сон по кнопке · ${longDay(run.night)}` : `Ночь на ${longDay(run.night)}`}
                        </h2>
                        <span class="w-cap">
                            {run.state === 'running' ? `Идёт, ${span}` : span}
                            {run.attempts > 1 ? ` · попыток: ${run.attempts}` : ''}
                            {night ? ` · обдумано ${night.thought} ${plural(night.thought, 'разговор', 'разговора', 'разговоров')}` : ''}
                        </span>
                    </div>
                    {manage && !run.rolledBackAt && run.state !== 'running' && live && (
                        <button class="w-btn rc-icon-btn" disabled={busy} onClick={() => void rollBack()}>
                            <IconUndo size={14} />
                            Откатить ночь
                        </button>
                    )}
                </div>
                {run.rolledBackAt && <p class="w-cap m-0">Ночь откачена в {clock(run.rolledBackAt)}: всё, что она сделала, вернулось, а её разговоры снова ждут рефлексии.</p>}
                {problem && (
                    <p class="w-cap c-dot-offline m-0" role="alert">
                        {problem}
                    </p>
                )}
                <div class="flex flex-col">
                    {run.parts.length === 0 && <p class="w-cap m-0">{run.state === 'running' ? 'Части сна ещё работают…' : 'Части сна ничего не сказали.'}</p>}
                    {run.parts.map((report) => (
                        <div key={report.part} class="rc-part">
                            <span class="rc-sleep-row">
                                <span class="w-strong">{report.title}</span>
                                <span class={`rc-state rc-state-${report.state}`}>{STATES[report.state]}</span>
                            </span>
                            <span class="w-cap">{report.summary}</span>
                            {report.lines.length > 0 && (
                                <ul class="rc-part-lines">
                                    {report.lines.map((line) => (
                                        <li key={line}>{line}</li>
                                    ))}
                                </ul>
                            )}
                        </div>
                    ))}
                </div>
            </section>
            {(order.length > 0 || notes.length > 0 || goneNotes.length > 0) && (
                <section class="card glass card-list" aria-labelledby="rc-night-ch-h">
                    <h2 id="rc-night-ch-h" class="t-title-xs m-0">
                        Что поменялось
                    </h2>
                    {order.length > 0 && <span class="rc-chg-grp">Порядок · {order.length}</span>}
                    {order.map((change) => {
                        const row = orderRow(change);
                        return (
                            <div key={change.id} class="rc-chg">
                                <span class="rc-chg-ico" aria-hidden="true">
                                    {row.icon}
                                </span>
                                <span class="min-w-0 flex flex-col">
                                    <span class={change.undoneAt ? 'rc-chg-title rc-undone' : 'rc-chg-title'}>{row.title}</span>
                                    {change.reason && <span class="rc-chg-note">{change.reason}</span>}
                                </span>
                                {undoButton(change)}
                            </div>
                        );
                    })}
                    {(notes.length > 0 || goneNotes.length > 0) && <span class="rc-chg-grp">Итоги дня · {notes.length}</span>}
                    {notes.map((note) => (
                        <div key={note.id} class="rc-chg">
                            <span class={`rc-chg-ico rc-${note.characterId ? 'pers' : 'home'}`} aria-hidden="true">
                                {note.characterId ? <IconMemory size={14} /> : <IconHome size={14} />}
                            </span>
                            <span class="min-w-0 flex flex-col">
                                <span class="rc-chg-title">{noteTitle(note)}</span>
                                <span class="rc-chg-quote">{note.text}</span>
                            </span>
                            {undoButton(noteChange(note))}
                        </div>
                    ))}
                    {goneNotes.map((change) => (
                        <div key={change.id} class="rc-chg">
                            <span class="rc-chg-ico" aria-hidden="true">
                                <IconPage size={14} />
                            </span>
                            <span class="min-w-0 flex flex-col">
                                <span class="rc-chg-title rc-undone">{change.summary || 'Итог или дневник'}</span>
                            </span>
                            <span class="w-cap">отменено</span>
                        </div>
                    ))}
                    <p class="w-cap m-0">Выводы, уроки персонажей и «на завтра» — следующим шагом рефлексии.</p>
                </section>
            )}
        </>
    );
}

// ───────── «Настройки» ─────────

/** «Модель памяти» в карточке «Память»: чем думает память — отдельно от «Мозга», дома или в облаке. */
export function MemoryModelSetting() {
    const panel = usePanel();
    const manage = can('memory.manage');
    const [view, setView] = useState<ReflectionView | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const reread = usePoll({
        every: 60_000,
        version: panel.version,
        read: (signal) =>
            api
                .reflection(signal)
                .then((fresh) => {
                    if (!signal.aborted) setView(fresh);
                })
                .catch((e) => {
                    if (!signal.aborted) setProblem(problemOf(e));
                }),
    });
    const choose = async (model: MemoryModel, cloud: boolean, title: string) => {
        if (cloud) {
            const sure = await ask({
                title: 'Думать в облаке?',
                text: `Разговоры, которые обдумывает память, будут уходить в ${title}. Модель дома обходится без этого.`,
                ok: 'Разрешить',
            });
            if (!sure) return;
        }
        setBusy(true);
        setProblem(null);
        api.setReflection({ model })
            .then(reread)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const options = view?.options ?? [];
    const current = view?.model;
    const found = current?.mode === 'pick' ? options.findIndex((o) => o.provider === current.provider && o.model === current.model) : -1;
    const value = !current ? 'home' : current.mode === 'pick' ? (found >= 0 ? `o${found}` : 'gone') : current.mode;
    const pick = (next: string) => {
        if (!view) return;
        if (next === 'home') void choose({ mode: 'home' }, false, '');
        else if (next === 'brain') void choose({ mode: 'brain' }, !!view.brain?.cloud, view.brain?.title ?? '«Мозг»');
        else {
            const option = options[Number(next.slice(1))];
            if (option) void choose({ mode: 'pick', provider: option.provider, model: option.model }, option.cloud, option.title);
        }
    };
    // Здесь же и выбирают: своя строка, а не «выберите в настройках памяти» из отчёта ночи.
    const line = !view
        ? 'Читаю…'
        : view.using
          ? `Сейчас думает ${view.using.title} · ${view.using.model} — ${view.using.cloud ? 'в облаке: обдуманное уходит туда.' : 'дома, наружу ничего не уходит.'}`
          : view.model.mode === 'home'
            ? 'Дома модели нет — память ждёт. Выберите облачную в списке или добавьте домашнюю.'
            : view.model.mode === 'brain'
              ? 'В «Мозге» модель не выбрана — память ждёт.'
              : 'Этой модели больше нет в «Мозге» — память ждёт. Выберите другую.';
    const group = (cloud: boolean) =>
        options.map((option, index) =>
            option.cloud === cloud ? (
                <option key={`${option.provider}:${option.model}`} value={`o${index}`}>
                    {option.title} · {option.model}
                </option>
            ) : null,
        );
    return (
        <div class="w-toggle rc-model-row">
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="w-strong">Модель памяти</span>
                <span class="w-cap">Думает за память: ночью — рефлексия, потом — разбор разговоров. Отдельно от «Мозга», дома или в облаке.</span>
                <span class="w-cap">{line}</span>
                {view && !view.using && view.model.mode === 'home' && !options.some((o) => !o.cloud) && (
                    <span class="w-cap">Домашнюю модель добавляют в «Персонаж» → «Мозг»: Ollama или LM Studio на этом компьютере.</span>
                )}
                {problem && (
                    <span class="w-cap c-dot-offline" role="alert">
                        {problem}
                    </span>
                )}
            </span>
            <select
                class="field rc-model-field"
                aria-label="Модель памяти"
                value={value}
                disabled={!manage || busy || !view}
                onChange={(event) => pick((event.target as HTMLSelectElement).value)}
            >
                <option value="home">Своя, дома{view?.home ? ` — ${view.home.title} · ${view.home.model}` : ''}</option>
                <option value="brain">Как у «Мозга»{view?.brain ? ` — ${view.brain.title} · ${view.brain.model}` : ''}</option>
                {options.some((o) => !o.cloud) && <optgroup label="Дома">{group(false)}</optgroup>}
                {options.some((o) => o.cloud) && <optgroup label="В облаке">{group(true)}</optgroup>}
                {value === 'gone' && current?.mode === 'pick' && (
                    <option value="gone" disabled>
                        {current.model} — нет в «Мозге»
                    </option>
                )}
            </select>
        </div>
    );
}

/** Карточка «Сон и рефлексия»: час сна, «Запустить сон сейчас», рефлексия и её части. */
export function SleepSettings({ onNight }: { onNight: (id: string) => void }) {
    const panel = usePanel();
    const manage = can('memory.manage');
    const [sleep, setSleep] = useState<SleepView | null>(null);
    const [reflection, setReflection] = useState<ReflectionView | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const reread = usePoll({
        every: 30_000,
        version: panel.version,
        read: (signal) =>
            Promise.all([api.sleep(signal), api.reflection(signal)])
                .then(([view, mind]) => {
                    if (signal.aborted) return;
                    setSleep(view);
                    setReflection(mind);
                })
                .catch((e) => {
                    if (!signal.aborted) setProblem(problemOf(e));
                }),
    });
    const act = (job: () => Promise<unknown>) => {
        setBusy(true);
        setProblem(null);
        job()
            .then(reread)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const off = !manage || busy || !reflection;
    // Модель выбирают тут же, в карточке «Память» выше: так и сказано, без «в настройках памяти».
    const waitingHere = reflection?.waiting ? 'Ждёт модель памяти — её выбирают выше, в карточке «Память».' : null;
    const thinker = !reflection
        ? ''
        : reflection.using
          ? ` Думает модель памяти: ${reflection.using.title} · ${reflection.using.model}${reflection.using.cloud ? ', в облаке' : ', дома'}.`
          : ` ${waitingHere ?? ''}`;
    const whyNot = sleep?.whyNot && sleep.whyNot === reflection?.waiting ? waitingHere : sleep?.whyNot;
    return (
        <section class="card glass card-list" aria-labelledby="rc-sleep-h">
            <div class="section-head">
                <h2 id="rc-sleep-h" class="t-title-xs m-0">
                    Сон и рефлексия
                </h2>
            </div>
            <div class="rc-toggles">
                <div class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">Сон</span>
                        <span class="w-cap">
                            Ночью, в тихий час по часам колонки{sleep ? ` (${sleep.zone})` : ''}, — если у включённой части есть работа.
                        </span>
                    </span>
                    <span class="rc-sleep-row">
                        <input
                            class="field rc-time-field"
                            type="time"
                            aria-label="Время сна"
                            value={sleep?.time ?? '04:00'}
                            disabled={off || !sleep}
                            onChange={(event) => {
                                const time = (event.target as HTMLInputElement).value;
                                if (time && time !== sleep?.time) act(() => api.setSleepTime(time));
                            }}
                        />
                    </span>
                </div>
                {manage && (
                    <div class="rc-sleep-row">
                        <button
                            class="w-btn rc-icon-btn"
                            disabled={busy || !sleep || !!sleep.running || !!sleep.whyNot}
                            onClick={() => act(() => api.runSleep().then(({ id }) => onNight(id)))}
                        >
                            <IconMoon size={14} />
                            {sleep?.running ? 'Сон идёт…' : 'Запустить сон сейчас'}
                        </button>
                        <span class="w-cap">
                            {whyNot ??
                                (reflection && reflection.pending > 0
                                    ? `Ждут рефлексии: ${reflection.pending} ${plural(reflection.pending, 'разговор', 'разговора', 'разговоров')}`
                                    : '')}
                        </span>
                    </div>
                )}
                <label class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">
                            Рефлексия<span class="rc-def">по умолчанию выключена</span>
                        </span>
                        <span class="w-cap">
                            Ночью обдумывает разговоры дня. Утром — «За ночь», любую перемену можно отменить, всю ночь — откатить.{thinker}
                        </span>
                    </span>
                    <Switch on={!!reflection?.enabled} label="Рефлексия" disabled={off} onChange={(on) => act(() => api.setReflection({ enabled: on }))} />
                </label>
                <div class="rc-parts">
                    <label class="w-toggle">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">Порядок</span>
                            <span class="w-cap">Склеить повторы, закрыть устаревшее, связать другие имена, «завтра» — в даты, прошедшее — в историю, важность.</span>
                        </span>
                        <Switch on={!!reflection?.order} label="Порядок" disabled={off} onChange={(on) => act(() => api.setReflection({ order: on }))} />
                    </label>
                    <label class="w-toggle">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">Итог дня</span>
                            <span class="w-cap">Короткий итог для дома и запись в дневник персонажа, который говорил, — его голосом.</span>
                        </span>
                        <Switch on={!!reflection?.day} label="Итог дня" disabled={off} onChange={(on) => act(() => api.setReflection({ day: on }))} />
                    </label>
                    <span class="w-cap">Выводы, уроки персонажей и «на завтра» — следующим шагом.</span>
                </div>
            </div>
            {problem && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {problem}
                </p>
            )}
        </section>
    );
}
