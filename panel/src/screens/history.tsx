// История приложения (docs/plans/package-history.md): лента записей — шаги черновика, установки, возвраты,
// объединения, — новые сверху. В мастерской — черновик: вернуть его к шагу, сравнить, объединить с другой версией,
// «взять» из прошлой то, что попросите агента. В окне приложения — что работает: вернуть версию, её данные или всё
// вместе, отменить возврат. Ничего не стирается: возврат — тоже запись, его можно отменить. Вид — простой; новый
// вид мастерской придёт из сессии нового визуала.

import { useEffect, useState } from 'preact/hooks';

import { HistoryComparison, HistoryFeed, HistoryPlace, HistoryRecord, HistoryRoom, MergeState, RestorePlan, RestoreWhat, api, problemOf } from '../api';
import { formatNumber, locale, tr } from '../i18n';
import { usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { ask } from '../ui/confirm';

type Mode = 'draft' | 'installed';
type Open = { id: string; panel: 'compare' | 'restore' | 'take' } | null;

const who = (): Record<string, string> => ({
    agent: tr.history.whoAgent,
    hand: tr.history.whoHand,
    voice: tr.history.whoVoice,
    mcp: tr.history.whoMcp,
    catalog: tr.history.whoCatalog,
    system: tr.history.whoSystem,
});

/** Когда: «12.11 15:02». */
export function when(at: string): string {
    return new Date(at).toLocaleString(locale(), { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
}

/** Кто: агент (голосом), человек по имени, каталог. */
function whoOf(record: HistoryRecord): string {
    const { who: by, name, via } = record.by;
    if (by === 'hand' && name) return name;
    if (by === 'agent' && via === 'voice') return tr.history.whoAgentVoice;
    return who()[by] ?? by;
}

/** Что за запись — словами. */
export function kindOf(record: HistoryRecord): string {
    switch (record.kind) {
        case 'step':
            return tr.history.kindStep;
        case 'install':
            return record.trial ? tr.history.kindTrial(record.version ?? '') : tr.history.kindInstalled(record.version ?? '');
        case 'restore':
            if (!record.what) return tr.history.kindDraftReturned;
            return record.what === 'data' ? tr.history.kindRestoreData : record.what === 'code' ? tr.history.kindRestoreCode(record.version ?? '') : tr.history.kindRestoreBoth(record.version ?? '');
        case 'merge':
            return tr.history.kindMerge;
        case 'catalog':
            return tr.history.kindCatalog(record.version ?? '');
        case 'copy':
            return tr.history.kindCopy(record.version ?? '');
        case 'found':
            return record.version ? tr.history.kindFoundVersion(record.version) : tr.history.kindFound;
        case 'data':
            return tr.history.kindData;
        case 'removed':
            return tr.history.kindRemoved;
    }
}

/** Запись — версия поставленного приложения (её можно вернуть). */
function isVersion(record: HistoryRecord): boolean {
    return ['install', 'catalog', 'copy', 'found'].includes(record.kind) || (record.kind === 'restore' && !!record.what && record.what !== 'data');
}

/** Разница двух версий: файлы и строки. */
function Comparison({ id, from, to }: { id: string; from: string; to?: string }) {
    const [result, setResult] = useState<HistoryComparison | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        api.compareHistory(id, from, to)
            .then(setResult)
            .catch((e) => setError(problemOf(e)));
    }, [id, from, to]);
    if (error) return <p class="t-caption m-0 c-dot-offline">{error}</p>;
    if (!result) return <p class="t-caption m-0">{tr.history.comparing}</p>;
    if (result.files.length === 0) return <p class="t-caption m-0">{tr.history.sameFiles}</p>;
    return (
        <div class="flex flex-col gap-8">
            {result.manifest.length > 0 && (
                <ul class="hist-manifest">
                    {result.manifest.map((line) => (
                        <li key={line} class="t-caption">
                            {line}
                        </li>
                    ))}
                </ul>
            )}
            {result.files.map((file) => (
                <div key={file.path} class="flex flex-col gap-4">
                    <span class="t-body-sm">
                        <span class="font-mono">{file.path}</span> — {file.change === 'added' ? tr.history.fileAdded : file.change === 'removed' ? tr.history.fileRemoved : tr.history.fileChanged}
                        {file.binary ? tr.history.notText : ''}
                    </span>
                    {file.lines.length > 0 && (
                        <pre class="hist-diff">
                            {file.lines.map((line, index) =>
                                line.op === '…' ? (
                                    <span key={index} class="hist-fold">
                                        {tr.history.foldedLines(line.text)}
                                    </span>
                                ) : (
                                    <span key={index} class={line.op === '+' ? 'hist-add' : line.op === '-' ? 'hist-del' : undefined}>
                                        {`${line.op} ${line.text}\n`}
                                    </span>
                                ),
                            )}
                        </pre>
                    )}
                </div>
            ))}
        </div>
    );
}

const whatTitles = (): Record<RestoreWhat, string> => ({
    code: tr.history.whatCode,
    'code+data': tr.history.whatBoth,
    data: tr.history.whatData,
});

const whatAbout = (): Record<RestoreWhat, string> => ({
    code: tr.history.aboutCode,
    'code+data': tr.history.aboutBoth,
    data: tr.history.aboutData,
});

/** Окно возврата: что вернуть, что будет, кнопка. */
function Restore({ id, record, onDone }: { id: string; record: HistoryRecord; onDone: (message: string) => void }) {
    const version = isVersion(record);
    const [what, setWhat] = useState<RestoreWhat>(version ? 'code' : 'data');
    const [plan, setPlan] = useState<RestorePlan | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    useEffect(() => {
        setPlan(null);
        setError(null);
        api.restorePlan(id, record.id, what)
            .then((next) => {
                setPlan(next);
                // Вид данных менялся: первым — «Код и данные» (решение по умолчанию 22 и его оговорка).
                if (next.risky && what === 'code') setWhat('code+data');
            })
            .catch((e) => setError(problemOf(e)));
    }, [id, record.id, what]);
    const choices: RestoreWhat[] = version ? ['code', 'code+data', 'data'] : ['data'];
    const go = async (approve: boolean) => {
        setBusy(true);
        setError(null);
        try {
            const report = await api.restorePackage(id, record.id, what, approve);
            if (!report.restored && report.needsApproval) {
                setPlan(report.needsApproval);
                setError(report.message);
                return;
            }
            onDone(report.message);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="hist-panel">
            <div class="hist-choice" role="radiogroup" aria-label={tr.history.whatToRestore}>
                {choices.map((choice) => (
                    <label key={choice}>
                        <input type="radio" name={`what-${record.id}`} checked={what === choice} disabled={busy || (choice !== 'data' && plan?.canCode === false)} onChange={() => setWhat(choice)} />
                        <span class="flex flex-col">
                            <span class="t-body-sm fw-700">{whatTitles()[choice]}</span>
                            <span class="t-caption">{whatAbout()[choice]}</span>
                        </span>
                    </label>
                ))}
            </div>
            {plan && (
                <ul class="hist-manifest">
                    {plan.lines.map((line) => (
                        <li key={line} class="t-caption">
                            {line}
                        </li>
                    ))}
                </ul>
            )}
            {plan?.risky && <p class="t-caption m-0 ws-hint">{tr.history.risky}</p>}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
            <div class="flex flex-wrap gap-8">
                {plan?.needsApproval ? (
                    <Action tone="on" label={tr.history.approveAndRestore} busy={busy} disabled={busy || !plan} onClick={() => void go(true)} />
                ) : (
                    <Action tone="on" label={tr.history.restore} busy={busy} disabled={busy || !plan} onClick={() => void go(false)} />
                )}
            </div>
        </div>
    );
}

/** «Взять из этой версии»: агент берёт из прошлой то, что попросили. */
function Take({ id, record, onDone }: { id: string; record: HistoryRecord; onDone: (message: string) => void }) {
    const [asked, setAsked] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const go = async () => {
        setBusy(true);
        setError(null);
        try {
            const report = await api.mergeDraft(id, record.id, true, asked.trim());
            onDone(report.message);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="hist-panel">
            <label class="flex flex-col gap-4">
                <span class="t-caption">{tr.history.takeAsk(when(record.at))}</span>
                <textarea class="field" rows={2} value={asked} placeholder={tr.history.takePlaceholder} onInput={(e) => setAsked((e.target as HTMLTextAreaElement).value)} />
            </label>
            {error && <p class="t-caption m-0 c-dot-offline">{error}</p>}
            <div>
                <Action tone="on" label={tr.history.take} busy={busy} disabled={busy || !asked.trim()} onClick={() => void go()} />
            </div>
        </div>
    );
}

/** Объединение, которое идёт: почему агент, спорные файлы, вопрос агента с вариантами, «Отменить». */
function MergeBanner({ id, merge, onChanged }: { id: string; merge: MergeState; onChanged: (message?: string) => void }) {
    const [busy, setBusy] = useState<string | null>(null);
    const [other, setOther] = useState('');
    const answer = async (choice: string) => {
        setBusy(choice);
        try {
            await api.answerMerge(id, choice);
            onChanged(tr.history.mergeAnswered);
        } catch (e) {
            onChanged(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const cancel = async () => {
        const yes = await ask({ title: tr.history.cancelMergeTitle, text: tr.history.cancelMergeText, ok: tr.history.cancelMergeOk });
        if (!yes) return;
        setBusy('cancel');
        try {
            await api.cancelMerge(id);
            onChanged(tr.history.mergeCancelled);
        } catch (e) {
            onChanged(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    return (
        <div class="hist-merge" role="status">
            <span class="t-body-sm">
                <b>{tr.history.merging}</b> {merge.reason}.
            </span>
            {merge.conflicts.length > 0 && <span class="t-caption">{tr.history.conflicts(merge.conflicts.map((conflict) => `${conflict.path} (${conflict.why})`).join('; '))}</span>}
            {merge.question ? (
                <div class="flex flex-col gap-8">
                    <span class="t-body-sm">
                        <span class="pill pill-agent">{tr.history.agentAsks}</span> {merge.question.text}
                    </span>
                    <div class="flex flex-wrap gap-8">
                        {merge.question.options.map((option) => (
                            <Action key={option} label={option} busy={busy === option} disabled={!!busy} onClick={() => void answer(option)} />
                        ))}
                    </div>
                    <div class="flex flex-wrap items-center gap-8">
                        <input class="field flex-1 min-w-0" value={other} placeholder={tr.history.ownWords} onInput={(e) => setOther((e.target as HTMLInputElement).value)} />
                        <Action label={tr.history.reply} busy={busy === other} disabled={!!busy || !other.trim()} onClick={() => void answer(other.trim())} />
                    </div>
                </div>
            ) : (
                <span class="t-caption">{tr.history.mergingNow}</span>
            )}
            <div>
                <Action label={tr.history.cancelMerge} busy={busy === 'cancel'} disabled={!!busy} onClick={() => void cancel()} />
            </div>
        </div>
    );
}

/** Лента истории приложения: в мастерской — черновика (`draft`), в окне приложения — того, что работает (`installed`). */
export function HistoryCard({ id, mode, onChanged }: { id: string; mode: Mode; onChanged?: () => void }) {
    const panel = usePanel();
    const [feed, setFeed] = useState<HistoryFeed | null>(null);
    const [older, setOlder] = useState<HistoryRecord[]>([]);
    const [merge, setMerge] = useState<MergeState | null>(null);
    const [open, setOpen] = useState<Open>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        api.packageHistory(id)
            .then((next) => {
                setFeed(next);
                setOlder([]);
                setError(null);
            })
            .catch((e) => setError(problemOf(e)));
        if (mode === 'draft')
            api.draftMerge(id)
                .then((answer) => setMerge(answer.merge))
                .catch(() => setMerge(null));
    }, [id, mode, panel.version]);
    const records = [...(feed?.records ?? []), ...older];
    const more = older.length > 0 ? older.length % 50 === 0 : (feed?.more ?? false);
    const done = (text: string, ok = true) => {
        setNote({ ok, text });
        setOpen(null);
        onChanged?.();
    };
    const run = async (key: string, work: () => Promise<string>) => {
        setBusy(key);
        try {
            done(await work());
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };
    const loadOlder = async () => {
        const last = records[records.length - 1];
        if (!last) return;
        const next = await api.packageHistory(id, last.id).catch(() => null);
        if (next) setOlder((list) => [...list, ...next.records]);
    };
    const toggle = (record: HistoryRecord, which: 'compare' | 'restore' | 'take') =>
        setOpen((now) => (now?.id === record.id && now.panel === which ? null : { id: record.id, panel: which }));
    const installed = feed?.marks.installed;
    if (error) return <p class="t-caption m-0 c-dot-offline">{error}</p>;
    if (!feed) return null;
    return (
        <section class="card glass card-list" aria-labelledby={`history-${mode}-h`}>
            <div class="card-head">
                <h2 id={`history-${mode}-h`} class="t-title-xs m-0">
                    {tr.history.title}
                </h2>
                <span class="t-meta">{records.length}{more ? '+' : ''}</span>
            </div>
            {mode === 'draft' && merge && <MergeBanner id={id} merge={merge} onChanged={(text) => done(text ?? tr.history.done)} />}
            {mode === 'draft' && !merge && feed.upstream && (
                <div class="hist-merge">
                    <span class="t-body-sm">
                        {tr.history.upstream(feed.upstream.title ?? feed.upstream.app, feed.upstream.version ?? tr.history.upstreamNewer)}
                    </span>
                    <div class="flex flex-wrap gap-8">
                        <Action tone="on" label={tr.history.mergeAction} busy={busy === 'upstream'} disabled={!!busy} onClick={() => void run('upstream', async () => (await api.mergeDraft(id, 'upstream')).message)} />
                        <Action label={tr.history.viaAgent} busy={busy === 'upstream-agent'} disabled={!!busy} onClick={() => void run('upstream-agent', async () => (await api.mergeDraft(id, 'upstream', true)).message)} />
                    </div>
                </div>
            )}
            {mode === 'installed' && feed.upstream && <p class="t-caption m-0">{tr.history.upstreamInstalled(feed.upstream.title ?? feed.upstream.app)}</p>}
            {records.length === 0 && <p class="t-caption m-0">{tr.history.empty}</p>}
            <ol class="hist-list">
                {records.map((record) => {
                    const isOpen = open?.id === record.id ? open.panel : null;
                    const canRestoreDraft = mode === 'draft' && !record.marks.includes('draft') && record.kind !== 'data' && record.kind !== 'removed';
                    // Поставленное приложение — и из мастерской: та же лента знает его версии.
                    const canRestore = !!installed && !record.marks.includes('installed') && (isVersion(record) || !!record.data);
                    const restoreLabel = isVersion(record) ? (mode === 'draft' ? tr.history.restoreInstalledDraft : tr.history.restoreShort) : tr.history.restoreData;
                    const undo = record.marks.includes('installed') && record.kind === 'restore' && !!record.what && (record.parents?.length ?? 0) > 0;
                    return (
                        <li key={record.id} class="hist-row">
                            <div class="hist-head">
                                <span class="t-body-sm fw-700" title={timeAgo(record.at)}>
                                    {when(record.at)}
                                </span>
                                <span class="t-caption">{kindOf(record).trim()}</span>
                                <span class={`pill ${record.by.who === 'agent' ? 'pill-agent' : 'pill-off'}`}>{whoOf(record)}</span>
                                {record.marks.includes('installed') && <span class="pill pill-own">{tr.history.working}</span>}
                                {mode === 'draft' && record.marks.includes('draft') && <span class="pill pill-off">{tr.history.draft}</span>}
                                {record.on === 'station' && <span class="t-meta">{tr.history.onStation}</span>}
                                {record.data && <span class="t-meta">{tr.history.plusData}</span>}
                                {record.check === 'failed' && <span class="t-caption c-dot-offline">{tr.history.checkFailed}</span>}
                            </div>
                            {record.asked && <span class="t-caption">{tr.history.asked(record.asked)}</span>}
                            {record.note && <span class="t-caption">{record.note}</span>}
                            <div class="hist-actions">
                                {record.kind !== 'data' && (
                                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => toggle(record, 'compare')}>
                                        {isOpen === 'compare' ? tr.history.hide : mode === 'draft' ? tr.history.compareDraft : tr.history.compareInstalled}
                                    </button>
                                )}
                                {canRestoreDraft && (
                                    <button
                                        class="t-caption cursor-pointer underline min-h-44"
                                        disabled={!!busy}
                                        onClick={async () => {
                                            const yes = await ask({ title: tr.history.restoreDraftTitle(when(record.at)), text: tr.history.restoreDraftText, ok: tr.history.restoreDraftOk });
                                            if (yes) void run(record.id, async () => (await api.restoreDraft(id, record.id), tr.history.draftRestored));
                                        }}
                                    >
                                        {tr.history.restoreDraftHere}
                                    </button>
                                )}
                                {mode === 'draft' && !record.inDraft && record.kind !== 'data' && (
                                    <button class="t-caption cursor-pointer underline min-h-44" disabled={!!busy} onClick={() => void run(record.id, async () => (await api.mergeDraft(id, record.id)).message)}>
                                        {tr.history.mergeIntoDraft}
                                    </button>
                                )}
                                {mode === 'draft' && record.inDraft && !record.marks.includes('draft') && record.kind !== 'data' && (
                                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => toggle(record, 'take')}>
                                        {isOpen === 'take' ? tr.history.hide : tr.history.takeFrom}
                                    </button>
                                )}
                                {canRestore && (
                                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => toggle(record, 'restore')}>
                                        {isOpen === 'restore' ? tr.history.hide : restoreLabel}
                                    </button>
                                )}
                                {undo && (
                                    <button
                                        class="t-caption cursor-pointer underline min-h-44"
                                        disabled={!!busy}
                                        onClick={() => void run(record.id, async () => (await api.restorePackage(id, record.parents![0], record.what ?? 'code')).message)}
                                    >
                                        {tr.history.undoRestore}
                                    </button>
                                )}
                            </div>
                            {isOpen === 'compare' && (
                                <div class="hist-panel">
                                    <Comparison id={id} from={record.id} to={mode === 'installed' ? installed : undefined} />
                                </div>
                            )}
                            {isOpen === 'restore' && <Restore id={id} record={record} onDone={(text) => done(text)} />}
                            {isOpen === 'take' && <Take id={id} record={record} onDone={(text) => done(text)} />}
                        </li>
                    );
                })}
            </ol>
            {more && (
                <div>
                    <Action label={tr.history.showEarlier} disabled={!!busy} onClick={() => void loadOlder()} />
                </div>
            )}
            {feed.earlierGone && !more && <p class="t-caption m-0">{tr.history.earlierGone}</p>}
            {note && (
                <p class={`t-caption m-0${note.ok ? '' : ' c-dot-offline'}`} role="status">
                    {note.text}
                </p>
            )}
        </section>
    );
}

const places = (): Record<HistoryPlace, string> => ({
    both: tr.history.placeBoth,
    home: tr.history.placeHome,
    here: tr.history.placeHere,
});

const mb = (bytes: number) => {
    const digits = bytes < 1024 * 1024 ? 2 : 1;
    return formatNumber(bytes / 1024 / 1024, { minimumFractionDigits: digits, maximumFractionDigits: digits });
};

/** «Память»: сколько места у истории приложений, где её хранить. Администратору. */
export function HistoryRoomLine({ titled = false }: { titled?: boolean }) {
    const panel = usePanel();
    const [room, setRoom] = useState<HistoryRoom | null>(null);
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<string | null>(null);
    useEffect(() => {
        api.historyRoom()
            .then(setRoom)
            .catch(() => setRoom(null));
    }, [panel.version]);
    if (!room) return null;
    const change = async (work: () => Promise<HistoryRoom>, done?: string) => {
        setBusy(true);
        try {
            setRoom(await work());
            if (done) setNote(done);
        } catch (e) {
            setNote(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="flex flex-col gap-8">
            <span class="t-caption">
                {tr.history.roomLine(titled ? tr.history.roomTaken : tr.history.roomHistory, mb(room.bytes), room.policy.budgetMb, room.policy.days, room.policy.steps)}
            </span>
            {room.places.length > 1 && (
                <label class="flex flex-wrap items-center gap-8">
                    <span class="t-caption">{tr.history.whereToKeep}</span>
                    <select class="field" value={room.policy.place} disabled={busy} onChange={(e) => void change(() => api.setHistorySettings({ place: (e.target as HTMLSelectElement).value as HistoryPlace }))}>
                        {room.places.map((place) => (
                            <option key={place} value={place}>
                                {place === 'here' ? tr.history.placeStationOnly : places()[place]}
                            </option>
                        ))}
                    </select>
                    {room.policy.place !== 'here' && !room.home && <span class="t-caption">{tr.history.computerOffline}</span>}
                </label>
            )}
            <div class="flex flex-wrap items-center gap-8">
                <label class="flex items-center gap-8">
                    <span class="t-caption">{tr.history.howMuchRoom}</span>
                    <input
                        class="field hist-number"
                        type="number"
                        min={1}
                        value={room.policy.budgetMb}
                        disabled={busy}
                        onChange={(e) => {
                            const value = Number((e.target as HTMLInputElement).value);
                            if (value >= 1) void change(() => api.setHistorySettings({ budgetMb: value }));
                        }}
                    />
                </label>
                <Action
                    label={tr.history.cleanNow}
                    busy={busy}
                    disabled={busy}
                    onClick={() =>
                        void change(async () => {
                            const cleaned = await api.cleanHistory();
                            setNote(tr.history.cleaned(cleaned.records, cleaned.objects));
                            return api.historyRoom();
                        })
                    }
                />
            </div>
            {note && <span class="t-caption">{note}</span>}
        </div>
    );
}
