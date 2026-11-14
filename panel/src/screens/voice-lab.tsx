import { useEffect, useRef, useState } from 'preact/hooks';

import { api, CloneEngineId, isTight, needsOf, problemOf, TtsCard, VoiceLabEngine, VoiceLabTake, VoiceLabView } from '../api';
import { useAudio } from '../audio';
import { IconMic, IconPlus, IconVoice } from '../icons';
import { fileToWav, MAX_TAKE_SECONDS, microphones, Recorder, recordingSupport } from '../recorder';
import { Action, Card, CardNote } from '../ui/cards';
import { Progress, useNeedsFlow, voicePill } from '../ui/needs';
import { Sheet } from '../ui/sheet';
import { ask } from '../ui/confirm';
import { formatNumber, tr } from '../i18n';
import { byMode } from '../words';

// «Свой голос»: отпечаток голоса владельца (zero-shot, без обучения) — мастер из трёх шагов и блок своих голосов
// на экране «Голос». Модель ставится кнопкой, дубли пишутся микрофоном или добавляются файлом. За памятью следит шлюз:
// впритык — предупреждает (можно «всё равно»), расчёт, который теснит колонку, останавливает. Своего лимита нет.
// Выбор голоса — как у любого: activateTts с id `own:<голос>`.

const fixed1 = (value: number): string => formatNumber(value, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
const gb = (mb: number): string => fixed1(mb / 1024);
const clock = (seconds: number): string => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`;

const devices = (): Record<string, string> => ({
    mps: tr.voiceLab.deviceMps,
    cuda: tr.voiceLab.deviceCuda,
    cpu: tr.voiceLab.deviceCpu,
});

const verdicts = (): Record<VoiceLabTake['verdict'], { text: string; tone: 'pill-own' | 'pill-off' | 'pill-yandex'; hint?: string }> => ({
    good: { text: tr.voiceLab.verdictGood, tone: 'pill-own' },
    short: { text: tr.voiceLab.verdictShort, tone: 'pill-off', hint: tr.voiceLab.hintShort },
    quiet: { text: tr.voiceLab.verdictQuiet, tone: 'pill-yandex', hint: tr.voiceLab.hintQuiet },
    noisy: { text: tr.voiceLab.verdictNoisy, tone: 'pill-yandex', hint: tr.voiceLab.hintNoisy },
    clipped: { text: tr.voiceLab.verdictClipped, tone: 'pill-yandex', hint: tr.voiceLab.hintClipped },
});

/** Полоса прогресса задачи: этап, процент, память. */
function JobBar({ label, percent, memoryMb }: { label: string; percent: number; memoryMb?: number }) {
    return (
        <div class="flex flex-col gap-6">
            <div class="flex items-center gap-10">
                <div role="progressbar" aria-label={label} aria-valuenow={percent} class="flex-1 h-8 r-pill bg-track overflow-hidden">
                    <div class="progress-fill" style={{ width: `${Math.max(3, percent)}%`, transition: 'width .4s' }} />
                </div>
                <span class="t-caption fw-700 flex-none">
                    {percent}%
                </span>
            </div>
            <span class="t-caption">
                {label}
                {memoryMb !== undefined ? tr.voiceLab.jobMemory(gb(memoryMb)) : ''}
            </span>
        </div>
    );
}

function Problem({ text }: { text?: string | null }) {
    if (!text) return null;
    return (
        <p class="t-caption m-0 c-dot-offline" role="alert">
            {text}
        </p>
    );
}

/** Предупреждение, которое можно пройти: памяти впритык. */
function Warning({ text }: { text?: string | null }) {
    if (!text) return null;
    return (
        <p class="t-caption m-0 c-pill-yandex-fg" role="status">
            {text}
        </p>
    );
}

/** Шапка панели «Свой голос»: что это, и «Создать» — мастер отпечатка. Под ней — снятые здесь голоса. */
export function OwnVoicesHeader({ isAdmin, hasOwn, onAdd }: { isAdmin: boolean; hasOwn: boolean; onAdd: () => void }) {
    return (
        <>
            <div class="flex items-center justify-between min-h-44 gap-8">
                <h2 class="t-title-xs m-0 inline-flex items-center gap-8">
                    <IconVoice size={18} />
                    {tr.voiceLab.ownVoice}
                </h2>
                {isAdmin && (
                    <button class="pill pill-own cursor-pointer py-8 px-14 min-h-36 inline-flex items-center gap-6" onClick={onAdd}>
                        <IconPlus size={14} />
                        {tr.voiceLab.create}
                    </button>
                )}
            </div>
            <p class="t-caption m-0 mb-6">
                {hasOwn
                    ? tr.voiceLab.hasOwnAbout
                    : byMode(tr.voiceLab.noOwnAboutSpeaker, tr.voiceLab.noOwnAboutVoice)}
            </p>
        </>
    );
}

/** Свои голоса строками в панели «Свой голос»: проба, выбор, имя, удаление. */
export function OwnVoiceRows({
    cards,
    isAdmin,
    notes,
    busy,
    onActivate,
    tightFor,
    onChanged,
}: {
    cards: TtsCard[];
    isAdmin: boolean;
    notes: Record<string, CardNote>;
    busy: Record<string, boolean>;
    onActivate: (cardId: string) => void;
    /** Голос, которому шлюз ответил «памяти впритык»: его кнопка — «включить всё равно». */
    tightFor?: string | null;
    onChanged: () => void;
}) {
    const audio = useAudio();
    const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
    const [problems, setProblems] = useState<Record<string, string>>({});

    const rename = async (card: TtsCard) => {
        if (!renaming || !card.own) return;
        try {
            await api.renameVoice(card.own.id, renaming.name);
            setRenaming(null);
            onChanged();
        } catch (error) {
            setProblems((all) => ({ ...all, [card.id]: problemOf(error) }));
        }
    };
    const remove = async (card: TtsCard) => {
        if (!card.own) return;
        const yes = await ask({ title: tr.voiceLab.removeTitle(card.title), text: tr.voiceLab.removeText, ok: tr.voiceLab.removeOk, danger: true });
        if (!yes) return;
        try {
            await api.deleteVoice(card.own.id);
            onChanged();
        } catch (error) {
            setProblems((all) => ({ ...all, [card.id]: problemOf(error) }));
        }
    };

    return (
        <>
            {cards.map((card) => {
                const own = card.own;
                const playing = audio.playing === `sample:${card.id}`;
                const editing = renaming && own && renaming.id === own.id;
                // Голос сам знает, чего ему не хватает: модель другой записи, ставится, не пойдёт здесь.
                const state = voicePill(card);
                return (
                    <Card
                        key={card.id}
                        title={card.title}
                        badge={card.badge}
                        description={card.description}
                        meta={own?.refSeconds ? tr.voiceLab.sampleSeconds(fixed1(own.refSeconds)) : undefined}
                        note={problems[card.id] ? { error: problems[card.id] } : notes[card.id]}
                        pills={state ? [state] : undefined}
                        action={
                            <div class="flex items-center gap-6 flex-wrap justify-end">
                                {own && own.samples > 0 && (
                                    <Action
                                        label={playing ? tr.voiceLab.stop : tr.voiceLab.play}
                                        onClick={() => void audio.toggle(`sample:${card.id}`, () => api.sampleAudio(own.id, 1))}
                                    />
                                )}
                                {card.active ? (
                                    <span class="pill pill-own">{tr.voiceLab.speaksNow}</span>
                                ) : (
                                    isAdmin &&
                                    card.state !== 'unsupported' && (
                                        <Action
                                            tone="on"
                                            label={tightFor === card.id ? tr.voiceLab.activateAnyway : tr.voiceLab.choose}
                                            busy={busy[card.id]}
                                            onClick={() => onActivate(card.id)}
                                        />
                                    )
                                )}
                            </div>
                        }
                    >
                        {card.state === 'installing' && card.progress !== undefined && <Progress percent={card.progress} label={tr.voiceLab.installing(card.title)} />}
                        {card.reason && card.state !== 'ready' && <span class="t-caption">{card.reason}</span>}
                        {isAdmin && own && !editing && (
                            <div class="flex gap-12">
                                <button class="t-caption cursor-pointer underline" onClick={() => setRenaming({ id: own.id, name: card.title })}>
                                    {tr.voiceLab.rename}
                                </button>
                                {!card.active && (
                                    <button class="t-caption cursor-pointer underline" onClick={() => void remove(card)}>
                                        {tr.voiceLab.remove}
                                    </button>
                                )}
                            </div>
                        )}
                        {editing && (
                            <div class="flex gap-8 items-center">
                                <input
                                    class="field flex-1 min-w-0"
                                    aria-label={tr.voiceLab.voiceNameLabel}
                                    maxLength={60}
                                    value={renaming.name}
                                    onInput={(event) => setRenaming({ id: own.id, name: (event.target as HTMLInputElement).value })}
                                    onKeyDown={(event) => {
                                        if (event.key === 'Enter') void rename(card);
                                        if (event.key === 'Escape') {
                                            // Esc отменяет имя, а не закрывает окно «Голос».
                                            event.preventDefault();
                                            setRenaming(null);
                                        }
                                    }}
                                />
                                <Action tone="on" label={tr.voiceLab.save} onClick={() => void rename(card)} />
                                <Action label={tr.voiceLab.cancel} onClick={() => setRenaming(null)} />
                            </div>
                        )}
                    </Card>
                );
            })}
        </>
    );
}

type Step = 1 | 2 | 3;

const GO = 'cursor-pointer py-10 px-18';
/** Шаг дальше пока закрыт: кнопка видна, но приглушена. */
const DIM = 'cursor-default py-10 px-18 opacity-45';

/** Мастер «Свой голос»: модель → запись → отпечаток. */
export function VoiceWizard({ onClose }: { onClose: () => void }) {
    const [lab, setLab] = useState<VoiceLabView | null>(null);
    const [loadError, setLoadError] = useState<string | null>(null);
    const [step, setStep] = useState<Step>(1);
    const [engine, setEngine] = useState<CloneEngineId | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [working, setWorking] = useState<string | null>(null);

    const refresh = async () => {
        try {
            const fresh = await api.voiceLab();
            setLab(fresh);
            setLoadError(null);
            return fresh;
        } catch (error) {
            setLoadError(problemOf(error));
            return null;
        }
    };

    useEffect(() => {
        let alive = true;
        let inFlight = false;
        const tick = () => {
            if (inFlight) return;
            inFlight = true;
            void refresh().finally(() => (inFlight = false));
        };
        tick();
        const id = setInterval(() => alive && tick(), 1500);
        return () => {
            alive = false;
            clearInterval(id);
        };
    }, []);

    // Модель по умолчанию — рекомендованная, если она здесь работает.
    useEffect(() => {
        if (!lab || engine) return;
        const fits = lab.engines.filter((entry) => entry.status !== 'unsupported');
        const pick = fits.find((entry) => entry.recommended) ?? fits[0];
        if (pick) setEngine(pick.id);
    }, [lab]);

    const act = async (what: string, run: () => Promise<unknown>) => {
        setProblem(null);
        setWorking(what);
        try {
            await run();
            await refresh();
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setWorking(null);
        }
    };

    const chosen = lab?.engines.find((entry) => entry.id === engine);
    const job = lab?.job;
    const imprintRunning = job?.kind === 'imprint' && job.state === 'running';
    const titles: Record<Step, string> = { 1: tr.voiceLab.stepModel, 2: tr.voiceLab.stepRecord, 3: tr.voiceLab.stepImprint };
    const step1Blocked = chosen?.status !== 'ready';
    const step2Blocked = !lab || lab.draft.seconds < 15;

    const footer = (
        <div class="flex gap-8 justify-between items-center">
            {step === 1 ? (
                <button class="pill pill-off cursor-pointer py-10 px-18" onClick={onClose}>
                    {tr.voiceLab.close}
                </button>
            ) : (
                <button class={`pill pill-off ${imprintRunning ? DIM : GO}`} disabled={imprintRunning} onClick={() => setStep((step - 1) as Step)}>
                    {tr.voiceLab.back}
                </button>
            )}
            {step === 1 && (
                <button class={`pill pill-own ${step1Blocked ? DIM : GO}`} disabled={step1Blocked} onClick={() => setStep(2)}>
                    {tr.voiceLab.next}
                </button>
            )}
            {step === 2 && (
                <button class={`pill pill-own ${step2Blocked ? DIM : GO}`} disabled={step2Blocked} onClick={() => setStep(3)}>
                    {tr.voiceLab.next}
                </button>
            )}
            {step === 3 && (
                <button class="pill pill-own cursor-pointer py-10 px-18" onClick={onClose}>
                    {tr.voiceLab.done}
                </button>
            )}
        </div>
    );

    return (
        <Sheet
            title={tr.voiceLab.ownVoice}
            about={tr.voiceLab.stepAbout(step, titles[step])}
            icon={
                <span class="dash-sheet-icon dash-circle inline-flex items-center justify-center">
                    <IconMic size={22} />
                </span>
            }
            onClose={onClose}
            footer={footer}
            dismissable={false}
        >
            {!lab ? (
                <p class="t-meta m-0" role={loadError ? 'alert' : undefined}>
                    {loadError ?? tr.voiceLab.checkingComputer}
                </p>
            ) : !lab.available ? (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {lab.reason}
                </p>
            ) : step === 1 ? (
                <EngineStep lab={lab} engine={engine} setEngine={setEngine} working={working} act={act} />
            ) : step === 2 ? (
                <RecordStep lab={lab} refresh={refresh} />
            ) : (
                <ImprintStep lab={lab} engine={engine!} working={working} act={act} />
            )}
            <Problem text={problem} />
        </Sheet>
    );
}

type Act = (what: string, run: () => Promise<unknown>) => Promise<void>;

/** Чего ждёт кнопка, пока идёт другая задача: одна тяжёлая задача за раз. */
function waitFor(lab: VoiceLabView, job: NonNullable<VoiceLabView['job']>): string {
    const title = lab.engines.find((entry) => entry.id === job.engine)?.title ?? job.engine;
    return job.kind === 'install' ? tr.voiceLab.waitInstall(title) : tr.voiceLab.waitImprint
}

function EngineStep({
    lab,
    engine,
    setEngine,
    working,
    act,
}: {
    lab: VoiceLabView;
    engine: CloneEngineId | null;
    setEngine: (id: CloneEngineId) => void;
    working: string | null;
    act: Act;
}) {
    const job = lab.job;
    const status = (entry: VoiceLabEngine) => {
        if (entry.status === 'unsupported') return entry.reason;
        if (entry.status === 'ready') return tr.voiceLab.engineReady(devices()[entry.device ?? 'cpu']);
        if (entry.status === 'installing') return undefined;
        return entry.device ? tr.voiceLab.engineMissingDevice(entry.diskGb, devices()[entry.device]) : tr.voiceLab.engineMissing(entry.diskGb);
    };
    return (
        <>
            <p class="t-caption m-0">
                {lab.platform?.summary}.{' '}
                <button class="t-caption cursor-pointer underline" onClick={() => void act('recheck', () => api.voiceLabRecheck())}>
                    {tr.voiceLab.recheck}
                </button>
            </p>
            <div role="radiogroup" aria-label={tr.voiceLab.engineGroupLabel} class="flex flex-col gap-8">
                {lab.engines.map((entry) => {
                    const selected = engine === entry.id;
                    const installing = job?.kind === 'install' && job.engine === entry.id && job.state === 'running';
                    const failed = job?.kind === 'install' && job.engine === entry.id && job.state !== 'running' && job.state !== 'done';
                    return (
                        <div
                            key={entry.id}
                            role="radio"
                            aria-checked={selected}
                            aria-disabled={entry.status === 'unsupported'}
                            tabIndex={entry.status === 'unsupported' ? -1 : 0}
                            class={`card glass py-12 px-14 flex flex-col gap-8 ${entry.status === 'unsupported' ? 'cursor-default opacity-55' : 'cursor-pointer'} ${selected ? 'outline-accent' : 'outline-none'}`}
                            onClick={() => entry.status !== 'unsupported' && setEngine(entry.id)}
                            onKeyDown={(event) => (event.key === ' ' || event.key === 'Enter') && entry.status !== 'unsupported' && setEngine(entry.id)}
                        >
                            <div class="flex items-center gap-8 flex-wrap">
                                <span class="fw-800">{entry.title}</span>
                                {entry.recommended && <span class="pill pill-agent">{tr.voiceLab.recommended}</span>}
                                <span class="t-caption">{entry.license}</span>
                            </div>
                            <span class="t-caption">{entry.about}</span>
                            {status(entry) && <span class="t-caption fw-700">{status(entry)}</span>}
                            {installing && job && <JobBar label={job.label} percent={job.percent} />}
                            {installing && job?.log.length ? (
                                <code class="t-caption opacity-70 nowrap overflow-hidden text-ellipsis">
                                    {job.log[job.log.length - 1]}
                                </code>
                            ) : null}
                            {failed && job?.error && <Problem text={job.error} />}
                            {entry.status === 'missing' && !installing && (
                                <div class="flex items-center gap-10 flex-wrap">
                                    <Action
                                        tone="on"
                                        label={working === `install:${entry.id}` ? tr.voiceLab.starting : tr.voiceLab.install}
                                        busy={working === `install:${entry.id}`}
                                        disabled={job?.state === 'running'}
                                        onClick={() => void act(`install:${entry.id}`, () => api.installEngine(entry.id))}
                                    />
                                    {job?.state === 'running' && <span class="t-caption">{waitFor(lab, job)}</span>}
                                </div>
                            )}
                            {installing && (
                                <div>
                                    <Action label={tr.voiceLab.stopJob} onClick={() => void act('stop', () => api.stopVoiceJob())} />
                                </div>
                            )}
                        </div>
                    );
                })}
            </div>
            {!lab.platform?.uv && lab.engines.some((entry) => entry.status === 'missing') && (
                <p class="t-caption m-0">
                    {tr.voiceLab.needUv}
                </p>
            )}
        </>
    );
}

function RecordStep({ lab, refresh }: { lab: VoiceLabView; refresh: () => Promise<VoiceLabView | null> }) {
    const audio = useAudio();
    const support = recordingSupport();
    const [passage, setPassage] = useState(() => Math.min(lab.draft.takes.length, lab.passages.length - 1));
    const [recorder, setRecorder] = useState<Recorder | null>(null);
    const [level, setLevel] = useState(-90);
    const [seconds, setSeconds] = useState(0);
    const [uploading, setUploading] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const [mics, setMics] = useState<Array<{ id: string; label: string }>>([]);
    const [mic, setMic] = useState<string>('');
    const recorderRef = useRef<Recorder | null>(null);
    const fileInput = useRef<HTMLInputElement>(null);

    useEffect(() => {
        if (support.ok) void microphones().then(setMics).catch(() => undefined);
        // Закрыли мастер посреди записи — микрофон отпускается.
        return () => recorderRef.current?.cancel();
    }, []);

    const upload = async (wav: Blob) => {
        setUploading(true);
        try {
            await api.addTake(wav, passage + 1);
            await refresh();
            setPassage((current) => Math.min(current + 1, lab.passages.length - 1));
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setUploading(false);
        }
    };

    const stop = async () => {
        const current = recorderRef.current;
        if (!current) return;
        recorderRef.current = null;
        setRecorder(null);
        const { wav } = await current.stop();
        await upload(wav);
    };

    const start = async () => {
        setProblem(null);
        try {
            const started = await Recorder.start(mic || undefined, (db, elapsed) => {
                setLevel(db);
                setSeconds(elapsed);
                if (elapsed >= MAX_TAKE_SECONDS) void stop();
            });
            recorderRef.current = started;
            setRecorder(started);
            setSeconds(0);
            void microphones().then(setMics).catch(() => undefined); // теперь с именами
        } catch (error) {
            setProblem((error as Error).message);
        }
    };

    const addFile = async (file: File | undefined) => {
        if (!file) return;
        setProblem(null);
        try {
            const { wav, seconds: length } = await fileToWav(file);
            if (length > 120) throw new Error(tr.voiceLab.fileTooLong);
            await upload(wav);
        } catch (error) {
            setProblem(error instanceof Error && !('status' in error) ? error.message : problemOf(error));
        }
    };

    const loud = Math.max(0, Math.min(100, ((level + 60) / 60) * 100));
    const takes = lab.draft.takes;
    return (
        <>
            <div class="card glass py-12 px-14 flex flex-col gap-6">
                <span class="fw-800">{tr.voiceLab.howToRecord}</span>
                <span class="t-caption">
                    {tr.voiceLab.howToRecordText}
                </span>
            </div>

            <div class="card glass py-12 px-14 flex flex-col gap-8">
                <div class="flex items-center justify-between">
                    <span class="t-meta">
                        {tr.voiceLab.passageOf(passage + 1, lab.passages.length)}
                    </span>
                    <span class="flex gap-6">
                        <Action label="←" onClick={() => setPassage((current) => Math.max(0, current - 1))} />
                        <Action label="→" onClick={() => setPassage((current) => Math.min(lab.passages.length - 1, current + 1))} />
                    </span>
                </div>
                <p class="m-0 fs-15 lh-22">
                    {lab.passages[passage]}
                </p>
            </div>

            {support.ok ? (
                <div class="flex flex-col gap-8">
                    {mics.length > 1 && !recorder && (
                        <select class="field" aria-label={tr.voiceLab.micLabel} value={mic} onChange={(event) => setMic((event.target as HTMLSelectElement).value)}>
                            <option value="">{tr.voiceLab.micDefault}</option>
                            {mics.map((entry) => (
                                <option key={entry.id} value={entry.id}>
                                    {entry.label}
                                </option>
                            ))}
                        </select>
                    )}
                    <div class="flex items-center gap-10">
                        <button
                            class={`${recorder ? 'pill pill-yandex' : 'pill pill-own'} cursor-pointer py-12 px-20 fw-800 inline-flex items-center gap-8`}
                            disabled={uploading}
                            onClick={() => void (recorder ? stop() : start())}
                        >
                            {uploading ? <span class="spinner spinner-sm" aria-hidden="true" /> : <IconMic size={16} />}
                            {recorder ? tr.voiceLab.stopRecording(clock(seconds)) : uploading ? tr.voiceLab.saving : tr.voiceLab.record}
                        </button>
                        {recorder && (
                            <div aria-label={tr.voiceLab.levelLabel} class="flex-1 h-8 r-pill bg-track overflow-hidden">
                                <div style={{ height: 8, width: `${loud}%`, background: level > -6 ? 'var(--dot-offline)' : 'var(--accent)', transition: 'width .1s' }} />
                            </div>
                        )}
                    </div>
                    {recorder && seconds > 0 && seconds < 3 && <span class="t-caption">{tr.voiceLab.speakMeter}</span>}
                </div>
            ) : (
                <p class="t-caption m-0">
                    {support.reason}
                </p>
            )}

            <div>
                <input ref={fileInput} type="file" accept="audio/*" hidden onChange={(event) => void addFile((event.target as HTMLInputElement).files?.[0])} />
                <Action label={tr.voiceLab.addFile} busy={uploading} onClick={() => fileInput.current?.click()} />
            </div>
            <Problem text={problem} />

            {takes.length > 0 && (
                <div class="flex flex-col">
                    <span class="t-meta py-4 px-0">
                        {tr.voiceLab.takes(clock(lab.draft.seconds))}
                    </span>
                    {takes.map((take) => {
                        const verdict = verdicts()[take.verdict];
                        const key = `take:${take.n}`;
                        return (
                            <div key={take.n} class="flex flex-col gap-4 bt-divider py-8 px-0">
                                <div class="flex items-center gap-8">
                                    <span class="fw-700 flex-1">
                                        {tr.voiceLab.take(take.n)}
                                        <span class="t-caption">
                                            {' '}
                                            · {clock(take.seconds)}
                                            {take.passage ? tr.voiceLab.takePassage(take.passage) : ''}
                                        </span>
                                    </span>
                                    <span class={`pill ${verdict.tone}`}>{verdict.text}</span>
                                    <Action label={audio.playing === key ? '■' : '▶'} onClick={() => void audio.toggle(key, () => api.takeAudio(take.n))} />
                                    <Action
                                        label="✕"
                                        onClick={() =>
                                            void api
                                                .removeTake(take.n)
                                                .then(refresh)
                                                .catch((error) => setProblem(problemOf(error)))
                                        }
                                    />
                                </div>
                                {verdict.hint && <span class="t-caption">{verdict.hint}</span>}
                            </div>
                        );
                    })}
                </div>
            )}
        </>
    );
}

function ImprintStep({ lab, engine, working, act }: { lab: VoiceLabView; engine: CloneEngineId; working: string | null; act: Act }) {
    const audio = useAudio();
    const [name, setName] = useState(tr.voiceLab.defaultName);
    const memory = lab.memory;
    const card = lab.engines.find((entry) => entry.id === engine);
    const title = card?.title ?? engine;
    const need = card?.needMb.imprint ?? 0;
    /** Шлюз сам сказал «впритык» (пульт мог видеть память чуть раньше): следующее нажатие — «всё равно». */
    const [warned, setWarned] = useState<string | null>(null);
    const [enabling, setEnabling] = useState<'busy' | 'done' | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    /** Голосу впритык по памяти: текст шлюза, следующее «Включить» — всё равно. */
    const [voiceTight, setVoiceTight] = useState<string | null>(null);
    /** «Снять ещё один»: результат прошлого отпечатка уходит из мастера. */
    const [putAside, setPutAside] = useState<string | null>(null);
    /** Голос включится сам, когда встанет модель: что сказать вместо «говорит вашим голосом». */
    const [pendingNote, setPendingNote] = useState<string | null>(null);
    /** Голосу не хватает модели (её убрали, пока снимался отпечаток): то же «нужна модель», внутри мастера. */
    const needs = useNeedsFlow((message) => {
        setEnabling('done');
        setProblem(null);
        setPendingNote(message);
    });
    const job = lab.job?.kind === 'imprint' && lab.job.id !== putAside ? lab.job : undefined;
    const voiceId = job?.state === 'done' ? job.result?.voiceId : undefined;
    const voice = voiceId ? lab.voices.find((entry) => entry.id === voiceId) : undefined;
    // Как решит шлюз: компьютеру уже не хватает памяти или отпечатку тесно рядом с запасом колонки — только предупредить,
    // снять всё равно решает владелец.
    const short = memory.pressure !== 'normal';
    const tight = !!card && memory.poolMb - memory.reserveMb < need;
    const anyway = short || tight || warned !== null;
    const tightText = short
        ? tr.voiceLab.tightShort
        : memory.poolMb >= need
            ? byMode(tr.voiceLab.tightSpeaker, tr.voiceLab.tightVoice)(gb(memory.poolMb), gb(need), gb(memory.reserveMb))
            : tr.voiceLab.tightLow(gb(memory.poolMb), gb(need));

    const start = () =>
        act('imprint', async () => {
            try {
                await api.imprint(engine, name.trim() || tr.voiceLab.defaultName, anyway);
                setWarned(null);
            } catch (error) {
                if (!isTight(error)) throw error;
                setWarned(problemOf(error));
            }
        });

    const enable = async () => {
        if (!voice) return;
        setProblem(null);
        setEnabling('busy');
        try {
            await api.activateTts(`own:${voice.id}`, undefined, voiceTight !== null);
            setVoiceTight(null);
            setPendingNote(null);
            setEnabling('done');
        } catch (error) {
            setEnabling(null);
            const wanted = needsOf(error);
            if (isTight(error)) setVoiceTight(problemOf(error));
            else if (wanted)
                needs.open({
                    who: voice.name,
                    needs: wanted,
                    ownId: voice.id,
                    retry: ({ install, anyway }) => api.activateTts(`own:${voice.id}`, undefined, anyway, install),
                });
            else setProblem(problemOf(error));
        }
    };

    if (voice)
        return (
            <>
                <p class="m-0 fw-800">{tr.voiceLab.readyVoice(voice.name)}</p>
                <span class="t-caption">
                    {voice.refSeconds ? tr.voiceLab.voiceTakenFrom(fixed1(voice.refSeconds)) : tr.voiceLab.voiceTakenFromUnknown}
                </span>
                {voice.samples.map((sample) => {
                    const key = `sample:${voice.id}:${sample.n}`;
                    return (
                        <div key={sample.n} class="flex items-center gap-10 bt-divider py-8 px-0">
                            <Action label={audio.playing === key ? '■' : '▶'} onClick={() => void audio.toggle(key, () => api.sampleAudio(voice.id, sample.n))} />
                            <span class="t-body-sm">{sample.text}</span>
                        </div>
                    );
                })}
                {enabling === 'done' ? (
                    <p class="t-caption m-0 c-accent-text">
                        {pendingNote ?? byMode(tr.voiceLab.speaksYourVoiceSpeaker, tr.voiceLab.speaksYourVoiceVoice)}
                        {tr.voiceLab.restoreOld}
                    </p>
                ) : needs.body ? (
                    <div class="flex flex-col gap-8">{needs.body}</div>
                ) : (
                    <div>
                        <Action
                            tone="on"
                            label={enabling === 'busy' ? tr.voiceLab.loadingModel : voiceTight ? tr.voiceLab.enableAnyway : tr.voiceLab.enableVoice}
                            busy={enabling === 'busy'}
                            onClick={() => void enable()}
                        />
                    </div>
                )}
                {enabling === 'busy' && <span class="t-caption">{tr.voiceLab.modelLoading}</span>}
                <Warning text={voiceTight && enabling !== 'busy' ? tr.voiceLab.voiceNotEnabled(voiceTight) : null} />
                <Problem text={problem} />
                <div>
                    <Action
                        label={tr.voiceLab.another}
                        onClick={() => {
                            setPutAside(lab.job?.id ?? null);
                            setEnabling(null);
                            setVoiceTight(null);
                            setPendingNote(null);
                            needs.close();
                        }}
                    />
                </div>
            </>
        );

    return (
        <>
            <label class="flex flex-col gap-6">
                <span class="t-meta">{tr.voiceLab.nameLabel}</span>
                <input class="field" maxLength={60} value={name} disabled={job?.state === 'running'} onInput={(event) => setName((event.target as HTMLInputElement).value)} />
            </label>

            <div class="card glass py-12 px-14 flex flex-col gap-4">
                <div class="flex items-center justify-between gap-8">
                    <span class="fw-800">{tr.voiceLab.memory}</span>
                    <span class="t-caption">
                        {tr.voiceLab.memoryFree(gb(memory.poolMb), gb(memory.totalMb))}
                    </span>
                </div>
                <span class="t-caption">
                    {byMode(tr.voiceLab.memoryNoteSpeaker, tr.voiceLab.memoryNoteVoice)(gb(need), gb(memory.reserveMb))}
                </span>
            </div>

            {job?.state === 'running' ? (
                <>
                    <JobBar label={job.label} percent={job.percent} memoryMb={job.memoryMb} />
                    <div>
                        <Action label={tr.voiceLab.stopJob} onClick={() => void act('stop', () => api.stopVoiceJob())} />
                    </div>
                </>
            ) : (
                <>
                    {job && job.state !== 'done' && <Problem text={job.error} />}
                    {card && anyway && (
                        <Warning text={tr.voiceLab.askAnyway(warned ?? tightText)} />
                    )}
                    <span class="t-caption">
                        {tr.voiceLab.imprintAbout(title)}
                    </span>
                    {lab.job?.state === 'running' && <span class="t-caption">{waitFor(lab, lab.job)}</span>}
                    <div>
                        <Action
                            tone="on"
                            label={working === 'imprint' ? tr.voiceLab.starting : anyway ? tr.voiceLab.imprintAnyway : tr.voiceLab.imprint}
                            busy={working === 'imprint'}
                            disabled={lab.job?.state === 'running'}
                            onClick={() => void start()}
                        />
                    </div>
                </>
            )}
        </>
    );
}
