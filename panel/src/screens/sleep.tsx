// «За ночь» и «Сон и рефлексия» (docs/plans/memory.md, «Сон и рефлексия»; макет — вкладка «За ночь» в дизайне памяти).
// Сон — ночная работа шлюза по частям; первая и пока единственная часть — рефлексия: ночью она обдумывает разговоры,
// наводит порядок в записях и пишет итог дня и дневник персонажа. Любую её перемену можно отменить, всю ночь — откатить.
// Рефлексия выключена по умолчанию (решение владельца 12.11). Смотреть — memory.view, менять — memory.manage.

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';

import {
    api,
    can,
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
import { formatDate, tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { Switch } from '../ui/switch';
import { clockOwner } from '../words';

/** «2026-11-13» → «13 ноября». */
function longDay(day: string): string {
    const [year, month, date] = day.split('-').map(Number);
    return year && month && date ? formatDate(new Date(year, month - 1, date), { day: 'numeric', month: 'long' }) : day;
}

/** «2026-11-13» → «13.11». */
function shortDay(day: string): string {
    const [year, month, date] = day.split('-').map(Number);
    return year && month && date ? formatDate(new Date(year, month - 1, date), { day: '2-digit', month: '2-digit' }) : day;
}

/** «4:07» по часам этого браузера. */
function clock(iso: string): string {
    return formatDate(new Date(iso), { hour: 'numeric', minute: '2-digit' });
}

function nightTitle(run: SleepRun): string {
    return run.trigger === 'manual' ? tr.sleep.nightManual(shortDay(run.night), clock(run.startedAt)) : tr.sleep.nightAt(shortDay(run.night));
}

/** Что ночь сделала — одной строкой для списка. */
function nightLine(run: SleepRun): string {
    if (run.state === 'running') return tr.sleep.running;
    if (run.rolledBackAt) return tr.sleep.rolledBack;
    const part = run.parts.find((report) => report.state !== 'nothing') ?? run.parts[0];
    return part ? part.summary : tr.sleep.didNothing;
}

const states = (): Record<SleepPartReport['state'], string> => ({
    done: tr.sleep.stateDone,
    nothing: tr.sleep.stateNothing,
    waiting: tr.sleep.stateWaiting,
    failed: tr.sleep.stateFailed,
});

/** Перемена «Порядка» — значком и словами. */
function orderRow(change: MemoryChange): { icon: JSX.Element; title: string } {
    const said = tr.sleep.quote(change.summary);
    switch (change.action) {
        case 'merge':
            return { icon: <IconLinks size={14} />, title: tr.sleep.orderMerge(said) };
        case 'close':
            return { icon: <IconClose size={14} />, title: tr.sleep.orderClose(said) };
        case 'past':
            return { icon: <IconJournal size={14} />, title: tr.sleep.orderPast(said) };
        case 'date':
            return { icon: <IconTimer size={14} />, title: tr.sleep.orderDate(said) };
        case 'alias':
            return { icon: <IconUser size={14} />, title: tr.sleep.orderAlias(change.summary) };
        case 'importance':
            return { icon: <IconSparkle size={14} />, title: tr.sleep.orderImportance(said) };
        default:
            return { icon: <IconPage size={14} />, title: tr.sleep.orderRewrite(said) };
    }
}

function noteTitle(note: SleepNote): string {
    return note.characterId ? tr.sleep.diary(note.characterTitle ?? note.characterId, shortDay(note.day)) : tr.sleep.homeSummary(shortDay(note.day));
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
                                {tr.sleep.offTitle}
                            </h2>
                            <span class="w-cap">
                                {tr.sleep.offAbout}
                            </span>
                        </div>
                        {manage && (
                            <button class="w-btn" disabled={busy} onClick={() => act(() => api.setReflection({ enabled: true }))}>
                                {tr.sleep.turnOn}
                            </button>
                        )}
                    </div>
                </section>
            )}
            <section class="card glass card-list" aria-labelledby="rc-nights-h">
                <div class="section-head">
                    <h2 id="rc-nights-h" class="t-title-xs m-0">
                        {tr.sleep.nights}
                    </h2>
                    {manage && (
                        <button
                            class="w-btn rc-icon-btn"
                            disabled={busy || !!running || !!sleep?.whyNot}
                            onClick={() => act(() => api.runSleep().then(({ id }) => onOpen(id)))}
                        >
                            <IconMoon size={14} />
                            {running ? tr.sleep.sleeping : tr.sleep.runNow}
                        </button>
                    )}
                </div>
                {sleep && (
                    <p class="w-cap m-0">
                        {tr.sleep.sleepAt(sleep.time, clockOwner())}{' '}
                        {running
                            ? tr.sleep.sleepingNow
                            : (sleep.whyNot ??
                              (reflection && reflection.pending > 0
                                  ? tr.sleep.pendingList(reflection.pending)
                                  : ''))}
                        {waitsForModel(reflection) && (
                            <>
                                {' '}
                                <button class="rc-link" onClick={onSettings}>
                                    {tr.sleep.pickModel}
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
                    <p class="w-cap m-0">{tr.sleep.reading}</p>
                ) : runs.length === 0 ? (
                    <p class="w-cap m-0">{tr.sleep.noNights}</p>
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
                                    {run.state === 'partial' && !run.rolledBackAt && <span class="rc-state rc-state-waiting">{tr.sleep.unfinished}</span>}
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
            {tr.sleep.allNights}
        </button>
    );
    if (!run) {
        return (
            <>
                {back}
                <p class={problem ? 'w-cap c-dot-offline' : 'w-cap'} role={problem ? 'alert' : undefined}>
                    {problem ?? tr.sleep.reading}
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
            title: tr.sleep.rollbackTitle,
            text: tr.sleep.rollbackText,
            ok: tr.sleep.rollbackOk,
        });
        if (sure) act(() => api.rollBackSleepRun(run.id));
    };
    const undo = (change: MemoryChange) => act(() => api.undoMemoryChange(change.id));
    const undoButton = (change: MemoryChange | undefined) =>
        !change ? (
            <span />
        ) : change.undoneAt ? (
            <span class="w-cap">{tr.sleep.undone}</span>
        ) : manage && !run.rolledBackAt ? (
            <button class="w-btn rc-undo" disabled={busy} onClick={() => undo(change)}>
                <IconUndo size={13} />
                {tr.sleep.undo}
            </button>
        ) : (
            <span />
        );
    const span = run.finishedAt ? `${clock(run.startedAt)}–${clock(run.finishedAt)}` : tr.sleep.spanSince(clock(run.startedAt));
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
                            {run.trigger === 'manual' ? tr.sleep.headManual(longDay(run.night)) : tr.sleep.headNight(longDay(run.night))}
                        </h2>
                        <span class="w-cap">
                            {run.state === 'running' ? tr.sleep.runningSpan(span) : span}
                            {run.attempts > 1 ? tr.sleep.attempts(run.attempts) : ''}
                            {night ? tr.sleep.thought(night.thought) : ''}
                        </span>
                    </div>
                    {manage && !run.rolledBackAt && run.state !== 'running' && live && (
                        <button class="w-btn rc-icon-btn" disabled={busy} onClick={() => void rollBack()}>
                            <IconUndo size={14} />
                            {tr.sleep.rollBackNight}
                        </button>
                    )}
                </div>
                {run.rolledBackAt && <p class="w-cap m-0">{tr.sleep.rolledBackAt(clock(run.rolledBackAt))}</p>}
                {problem && (
                    <p class="w-cap c-dot-offline m-0" role="alert">
                        {problem}
                    </p>
                )}
                <div class="flex flex-col">
                    {run.parts.length === 0 && <p class="w-cap m-0">{run.state === 'running' ? tr.sleep.partsRunning : tr.sleep.partsSilent}</p>}
                    {run.parts.map((report) => (
                        <div key={report.part} class="rc-part">
                            <span class="rc-sleep-row">
                                <span class="w-strong">{report.title}</span>
                                <span class={`rc-state rc-state-${report.state}`}>{states()[report.state]}</span>
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
                        {tr.sleep.changed}
                    </h2>
                    {order.length > 0 && <span class="rc-chg-grp">{tr.sleep.orderGroup(order.length)}</span>}
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
                    {(notes.length > 0 || goneNotes.length > 0) && <span class="rc-chg-grp">{tr.sleep.notesGroup(notes.length)}</span>}
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
                                <span class="rc-chg-title rc-undone">{change.summary || tr.sleep.summaryOrDiary}</span>
                            </span>
                            <span class="w-cap">{tr.sleep.undone}</span>
                        </div>
                    ))}
                    <p class="w-cap m-0">{tr.sleep.nextStep}</p>
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
                title: tr.sleep.cloudTitle,
                text: tr.sleep.cloudText(title),
                ok: tr.sleep.cloudOk,
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
        else if (next === 'brain') void choose({ mode: 'brain' }, !!view.brain?.cloud, view.brain?.title ?? tr.sleep.brainName);
        else {
            const option = options[Number(next.slice(1))];
            if (option) void choose({ mode: 'pick', provider: option.provider, model: option.model }, option.cloud, option.title);
        }
    };
    // Здесь же и выбирают: своя строка, а не «выберите в настройках памяти» из отчёта ночи.
    const line = !view
        ? tr.sleep.reading
        : view.using
          ? tr.sleep.nowThinking(view.using.title, view.using.model, view.using.cloud ? tr.sleep.whereCloud : tr.sleep.whereHome)
          : view.model.mode === 'home'
            ? tr.sleep.noHomeModel
            : view.model.mode === 'brain'
              ? tr.sleep.noBrainModel
              : tr.sleep.modelGone;
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
                <span class="w-strong">{tr.sleep.modelTitle}</span>
                <span class="w-cap">{tr.sleep.modelAbout}</span>
                <span class="w-cap">{line}</span>
                {view && !view.using && view.model.mode === 'home' && !options.some((o) => !o.cloud) && (
                    <span class="w-cap">{tr.sleep.addHomeModel}</span>
                )}
                {problem && (
                    <span class="w-cap c-dot-offline" role="alert">
                        {problem}
                    </span>
                )}
            </span>
            <select
                class="field rc-model-field"
                aria-label={tr.sleep.modelTitle}
                value={value}
                disabled={!manage || busy || !view}
                onChange={(event) => pick((event.target as HTMLSelectElement).value)}
            >
                <option value="home">{tr.sleep.optionHome}{view?.home ? ` — ${view.home.title} · ${view.home.model}` : ''}</option>
                <option value="brain">{tr.sleep.optionBrain}{view?.brain ? ` — ${view.brain.title} · ${view.brain.model}` : ''}</option>
                {options.some((o) => !o.cloud) && <optgroup label={tr.sleep.groupHome}>{group(false)}</optgroup>}
                {options.some((o) => o.cloud) && <optgroup label={tr.sleep.groupCloud}>{group(true)}</optgroup>}
                {value === 'gone' && current?.mode === 'pick' && (
                    <option value="gone" disabled>
                        {tr.sleep.notInBrain(current.model)}
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
    const waitingHere = reflection?.waiting ? tr.sleep.waitingHere : null;
    const thinker = !reflection
        ? ''
        : reflection.using
          ? tr.sleep.thinkerUsing(reflection.using.title, reflection.using.model, reflection.using.cloud)
          : ` ${waitingHere ?? ''}`;
    const whyNot = sleep?.whyNot && sleep.whyNot === reflection?.waiting ? waitingHere : sleep?.whyNot;
    return (
        <section class="card glass card-list" aria-labelledby="rc-sleep-h">
            <div class="section-head">
                <h2 id="rc-sleep-h" class="t-title-xs m-0">
                    {tr.sleep.settingsTitle}
                </h2>
            </div>
            <div class="rc-toggles">
                <div class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{tr.sleep.sleepLabel}</span>
                        <span class="w-cap">{tr.sleep.sleepAbout(clockOwner(), sleep ? sleep.zone : null)}</span>
                    </span>
                    <span class="rc-sleep-row">
                        <input
                            class="field rc-time-field"
                            type="time"
                            aria-label={tr.sleep.sleepTimeLabel}
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
                            {sleep?.running ? tr.sleep.sleeping : tr.sleep.runNow}
                        </button>
                        <span class="w-cap">
                            {whyNot ??
                                (reflection && reflection.pending > 0
                                    ? tr.sleep.pendingBare(reflection.pending)
                                    : '')}
                        </span>
                    </div>
                )}
                <label class="w-toggle">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">
                            {tr.sleep.reflection}
                            <span class="rc-def">{tr.sleep.reflectionDefault}</span>
                        </span>
                        <span class="w-cap">
                            {tr.sleep.reflectionAbout}
                            {thinker}
                        </span>
                    </span>
                    <Switch on={!!reflection?.enabled} label={tr.sleep.reflection} disabled={off} onChange={(on) => act(() => api.setReflection({ enabled: on }))} />
                </label>
                <div class="rc-parts">
                    <label class="w-toggle">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">{tr.sleep.partOrder}</span>
                            <span class="w-cap">{tr.sleep.partOrderAbout}</span>
                        </span>
                        <Switch on={!!reflection?.order} label={tr.sleep.partOrder} disabled={off} onChange={(on) => act(() => api.setReflection({ order: on }))} />
                    </label>
                    <label class="w-toggle">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">{tr.sleep.partDay}</span>
                            <span class="w-cap">{tr.sleep.partDayAbout}</span>
                        </span>
                        <Switch on={!!reflection?.day} label={tr.sleep.partDay} disabled={off} onChange={(on) => act(() => api.setReflection({ day: on }))} />
                    </label>
                    <span class="w-cap">{tr.sleep.nextStepShort}</span>
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
