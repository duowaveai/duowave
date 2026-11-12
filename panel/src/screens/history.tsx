// История приложения (docs/plans/package-history.md): лента записей — шаги черновика, установки, возвраты,
// объединения, — новые сверху. В мастерской — черновик: вернуть его к шагу, сравнить, объединить с другой версией,
// «взять» из прошлой то, что попросите агента. В окне приложения — что работает: вернуть версию, её данные или всё
// вместе, отменить возврат. Ничего не стирается: возврат — тоже запись, его можно отменить. Вид — простой; новый
// вид мастерской придёт из сессии нового визуала.

import { useEffect, useState } from 'preact/hooks';

import { HistoryComparison, HistoryFeed, HistoryPlace, HistoryRecord, HistoryRoom, MergeState, RestorePlan, RestoreWhat, api, problemOf } from '../api';
import { usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { ask } from '../ui/confirm';

type Mode = 'draft' | 'installed';
type Open = { id: string; panel: 'compare' | 'restore' | 'take' } | null;

const WHO: Record<string, string> = {
    agent: 'агент',
    hand: 'руками',
    voice: 'голосом',
    mcp: 'Claude Code',
    catalog: 'каталог',
    system: 'шлюз',
};

/** Когда: «12.11 15:02». */
export function when(at: string): string {
    return new Date(at).toLocaleString('ru-RU', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
}

/** Кто: агент (голосом), человек по имени, каталог. */
function whoOf(record: HistoryRecord): string {
    const { who, name, via } = record.by;
    if (who === 'hand' && name) return name;
    if (who === 'agent' && via === 'voice') return 'агент, по просьбе голосом';
    return WHO[who] ?? who;
}

/** Что за запись — словами. */
export function kindOf(record: HistoryRecord): string {
    switch (record.kind) {
        case 'step':
            return 'шаг';
        case 'install':
            return record.trial ? `проба ${record.version ?? ''}` : `поставлена ${record.version ?? ''}`;
        case 'restore':
            if (!record.what) return 'черновик возвращён';
            return record.what === 'data' ? 'возврат данных' : record.what === 'code' ? `возврат к ${record.version ?? ''}` : `возврат к ${record.version ?? ''} с данными`;
        case 'merge':
            return 'объединение';
        case 'catalog':
            return `из каталога ${record.version ?? ''}`;
        case 'copy':
            return `своя копия ${record.version ?? ''}`;
        case 'found':
            return record.version ? `найдено на диске ${record.version}` : 'найдено на диске';
        case 'data':
            return 'снимок данных';
        case 'removed':
            return 'удалено';
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
    if (!result) return <p class="t-caption m-0">Сравниваю…</p>;
    if (result.files.length === 0) return <p class="t-caption m-0">Файлы те же.</p>;
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
                        <span class="font-mono">{file.path}</span> — {file.change === 'added' ? 'новый' : file.change === 'removed' ? 'убран' : 'изменён'}
                        {file.binary ? ', не текст' : ''}
                    </span>
                    {file.lines.length > 0 && (
                        <pre class="hist-diff">
                            {file.lines.map((line, index) =>
                                line.op === '…' ? (
                                    <span key={index} class="hist-fold">
                                        {`… ${line.text} строк без изменений\n`}
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

const WHAT_TITLES: Record<RestoreWhat, string> = {
    code: 'Только код',
    'code+data': 'Код и данные',
    data: 'Только данные',
};

const WHAT_ABOUT: Record<RestoreWhat, string> = {
    code: 'Вернётся старая версия, сегодняшние данные останутся.',
    'code+data': 'Вернётся старая версия и её данные той поры; что появилось потом, сохранится в истории.',
    data: 'Код останется этим, данные вернутся к той поре.',
};

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
            <div class="hist-choice" role="radiogroup" aria-label="Что вернуть">
                {choices.map((choice) => (
                    <label key={choice}>
                        <input type="radio" name={`what-${record.id}`} checked={what === choice} disabled={busy || (choice !== 'data' && plan?.canCode === false)} onChange={() => setWhat(choice)} />
                        <span class="flex flex-col">
                            <span class="t-body-sm fw-700">{WHAT_TITLES[choice]}</span>
                            <span class="t-caption">{WHAT_ABOUT[choice]}</span>
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
            {plan?.risky && <p class="t-caption m-0 ws-hint">Вид данных с тех пор менялся: старая версия может не понять сегодняшние данные.</p>}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
            <div class="flex flex-wrap gap-8">
                {plan?.needsApproval ? (
                    <Action tone="on" label="Одобрить права и вернуть" busy={busy} disabled={busy || !plan} onClick={() => void go(true)} />
                ) : (
                    <Action tone="on" label="Вернуть" busy={busy} disabled={busy || !plan} onClick={() => void go(false)} />
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
                <span class="t-caption">Что взять из версии от {when(record.at)}? Агент возьмёт это в черновик, а если будет непонятно — спросит.</span>
                <textarea class="field" rows={2} value={asked} placeholder="Например: прогноз на завтра, как там было" onInput={(e) => setAsked((e.target as HTMLTextAreaElement).value)} />
            </label>
            {error && <p class="t-caption m-0 c-dot-offline">{error}</p>}
            <div>
                <Action tone="on" label="Взять" busy={busy} disabled={busy || !asked.trim()} onClick={() => void go()} />
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
            onChanged('Ответ ушёл агенту — он продолжает объединение.');
        } catch (e) {
            onChanged(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const cancel = async () => {
        const yes = await ask({ title: 'Отменить объединение?', text: 'Черновик вернётся к шагу до объединения.', ok: 'Отменить' });
        if (!yes) return;
        setBusy('cancel');
        try {
            await api.cancelMerge(id);
            onChanged('Объединение отменено.');
        } catch (e) {
            onChanged(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    return (
        <div class="hist-merge" role="status">
            <span class="t-body-sm">
                <b>Объединение:</b> {merge.reason}.
            </span>
            {merge.conflicts.length > 0 && <span class="t-caption">Спорные файлы: {merge.conflicts.map((conflict) => `${conflict.path} (${conflict.why})`).join('; ')}</span>}
            {merge.question ? (
                <div class="flex flex-col gap-8">
                    <span class="t-body-sm">
                        <span class="pill pill-agent">агент спрашивает</span> {merge.question.text}
                    </span>
                    <div class="flex flex-wrap gap-8">
                        {merge.question.options.map((option) => (
                            <Action key={option} label={option} busy={busy === option} disabled={!!busy} onClick={() => void answer(option)} />
                        ))}
                    </div>
                    <div class="flex flex-wrap items-center gap-8">
                        <input class="field flex-1 min-w-0" value={other} placeholder="Или своими словами" onInput={(e) => setOther((e.target as HTMLInputElement).value)} />
                        <Action label="Ответить" busy={busy === other} disabled={!!busy || !other.trim()} onClick={() => void answer(other.trim())} />
                    </div>
                </div>
            ) : (
                <span class="t-caption">Агент объединяет: что объединилось само, уже в черновике. Если будет непонятно, что оставить, — спросит здесь.</span>
            )}
            <div>
                <Action label="Отменить объединение" busy={busy === 'cancel'} disabled={!!busy} onClick={() => void cancel()} />
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
                    История
                </h2>
                <span class="t-meta">{records.length}{more ? '+' : ''}</span>
            </div>
            {mode === 'draft' && merge && <MergeBanner id={id} merge={merge} onChanged={(text) => done(text ?? 'Готово.')} />}
            {mode === 'draft' && !merge && feed.upstream && (
                <div class="hist-merge">
                    <span class="t-body-sm">
                        В «{feed.upstream.title ?? feed.upstream.app}» вышла версия {feed.upstream.version ?? 'новее'}. В вашей копии свои правки — объединить?
                    </span>
                    <div class="flex flex-wrap gap-8">
                        <Action tone="on" label="Объединить" busy={busy === 'upstream'} disabled={!!busy} onClick={() => void run('upstream', async () => (await api.mergeDraft(id, 'upstream')).message)} />
                        <Action label="Через агента" busy={busy === 'upstream-agent'} disabled={!!busy} onClick={() => void run('upstream-agent', async () => (await api.mergeDraft(id, 'upstream', true)).message)} />
                    </div>
                </div>
            )}
            {mode === 'installed' && feed.upstream && <p class="t-caption m-0">Вышла новая версия «{feed.upstream.title ?? feed.upstream.app}»: объединить её с этой копией можно в мастерской.</p>}
            {records.length === 0 && <p class="t-caption m-0">Истории пока нет: она начнётся с первого шага или установки.</p>}
            <ol class="hist-list">
                {records.map((record) => {
                    const isOpen = open?.id === record.id ? open.panel : null;
                    const canRestoreDraft = mode === 'draft' && !record.marks.includes('draft') && record.kind !== 'data' && record.kind !== 'removed';
                    // Поставленное приложение — и из мастерской: та же лента знает его версии.
                    const canRestore = !!installed && !record.marks.includes('installed') && (isVersion(record) || !!record.data);
                    const restoreLabel = isVersion(record) ? (mode === 'draft' ? 'вернуть поставленную…' : 'вернуть…') : 'вернуть данные…';
                    const undo = record.marks.includes('installed') && record.kind === 'restore' && !!record.what && (record.parents?.length ?? 0) > 0;
                    return (
                        <li key={record.id} class="hist-row">
                            <div class="hist-head">
                                <span class="t-body-sm fw-700" title={timeAgo(record.at)}>
                                    {when(record.at)}
                                </span>
                                <span class="t-caption">{kindOf(record).trim()}</span>
                                <span class={`pill ${record.by.who === 'agent' ? 'pill-agent' : 'pill-off'}`}>{whoOf(record)}</span>
                                {record.marks.includes('installed') && <span class="pill pill-own">работает</span>}
                                {mode === 'draft' && record.marks.includes('draft') && <span class="pill pill-off">черновик</span>}
                                {record.on === 'station' && <span class="t-meta">на колонке</span>}
                                {record.data && <span class="t-meta">+ данные</span>}
                                {record.check === 'failed' && <span class="t-caption c-dot-offline">проверка не прошла</span>}
                            </div>
                            {record.asked && <span class="t-caption">Просили: «{record.asked}»</span>}
                            {record.note && <span class="t-caption">{record.note}</span>}
                            <div class="hist-actions">
                                {record.kind !== 'data' && (
                                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => toggle(record, 'compare')}>
                                        {isOpen === 'compare' ? 'скрыть' : mode === 'draft' ? 'сравнить с черновиком' : 'сравнить с работающей'}
                                    </button>
                                )}
                                {canRestoreDraft && (
                                    <button
                                        class="t-caption cursor-pointer underline min-h-44"
                                        disabled={!!busy}
                                        onClick={async () => {
                                            const yes = await ask({ title: `Вернуть черновик к ${when(record.at)}?`, text: 'Это будет новый шаг: сегодняшнее останется в истории, к нему можно вернуться.', ok: 'Вернуть' });
                                            if (yes) void run(record.id, async () => (await api.restoreDraft(id, record.id), 'Черновик возвращён. Вернуться обратно — тем же способом.'));
                                        }}
                                    >
                                        вернуть черновик сюда
                                    </button>
                                )}
                                {mode === 'draft' && !record.inDraft && record.kind !== 'data' && (
                                    <button class="t-caption cursor-pointer underline min-h-44" disabled={!!busy} onClick={() => void run(record.id, async () => (await api.mergeDraft(id, record.id)).message)}>
                                        объединить с черновиком
                                    </button>
                                )}
                                {mode === 'draft' && record.inDraft && !record.marks.includes('draft') && record.kind !== 'data' && (
                                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => toggle(record, 'take')}>
                                        {isOpen === 'take' ? 'скрыть' : 'взять из неё…'}
                                    </button>
                                )}
                                {canRestore && (
                                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => toggle(record, 'restore')}>
                                        {isOpen === 'restore' ? 'скрыть' : restoreLabel}
                                    </button>
                                )}
                                {undo && (
                                    <button
                                        class="t-caption cursor-pointer underline min-h-44"
                                        disabled={!!busy}
                                        onClick={() => void run(record.id, async () => (await api.restorePackage(id, record.parents![0], record.what ?? 'code')).message)}
                                    >
                                        отменить этот возврат
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
                    <Action label="Показать раньше" disabled={!!busy} onClick={() => void loadOlder()} />
                </div>
            )}
            {feed.earlierGone && !more && <p class="t-caption m-0">Раньше — удалено по сроку.</p>}
            {note && (
                <p class={`t-caption m-0${note.ok ? '' : ' c-dot-offline'}`} role="status">
                    {note.text}
                </p>
            )}
        </section>
    );
}

const PLACES: Record<HistoryPlace, string> = {
    both: 'И на колонке, и на компьютере',
    home: 'На компьютере',
    here: 'Здесь',
};

const mb = (bytes: number) => (bytes / 1024 / 1024).toFixed(bytes < 1024 * 1024 ? 2 : 1);

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
                {titled ? 'Занято' : 'История приложений:'} {mb(room.bytes)} МБ из {room.policy.budgetMb} МБ · шаги хранятся {room.policy.days} дн., до {room.policy.steps} на приложение
            </span>
            {room.places.length > 1 && (
                <label class="flex flex-wrap items-center gap-8">
                    <span class="t-caption">Где хранить историю:</span>
                    <select class="field" value={room.policy.place} disabled={busy} onChange={(e) => void change(() => api.setHistorySettings({ place: (e.target as HTMLSelectElement).value as HistoryPlace }))}>
                        {room.places.map((place) => (
                            <option key={place} value={place}>
                                {place === 'here' ? 'Только на колонке' : PLACES[place]}
                            </option>
                        ))}
                    </select>
                    {room.policy.place !== 'here' && !room.home && <span class="t-caption">компьютер сейчас не на связи — история подождёт</span>}
                </label>
            )}
            <div class="flex flex-wrap items-center gap-8">
                <label class="flex items-center gap-8">
                    <span class="t-caption">Сколько места, МБ:</span>
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
                    label="Почистить сейчас"
                    busy={busy}
                    disabled={busy}
                    onClick={() =>
                        void change(async () => {
                            const cleaned = await api.cleanHistory();
                            setNote(`Убрано записей: ${cleaned.records}, файлов: ${cleaned.objects}.`);
                            return api.historyRoom();
                        })
                    }
                />
            </div>
            {note && <span class="t-caption">{note}</span>}
        </div>
    );
}
