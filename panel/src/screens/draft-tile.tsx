// Плитка-черновик (docs/plans/voice-creation.md, «Пульт: плитка, комета, «Примерка», посадка»; макет
// docs/design/voice-creation, шаги «Агент в фоне» и «Готово»). Вещь рождается там, где будет стоять: плитка «Новый
// виджет» становится ею. Пока агент работает — фиолетовый пунктирный контур (его рисует комета агента, ui/comet.ts),
// внутри по настоящим шагам агента проявляются строки. Готово — пунктир остаётся (это черновик), метка «на примерке»,
// внутри пример данных, под плиткой «Примерить» и «На стол». Настоящим листом плитка станет только на столе, после
// «да». Код черновика работает в своём окошке (docs/plans/bus.md, «Окошки»), поэтому показывается сразу — и черновик,
// попрошенный у колонки, тоже.

import { useEffect, useRef, useState } from 'preact/hooks';

import { AgentStep, DraftView, api, problemOf } from '../api';
import { IconCheck, IconPlus, IconSparkle } from '../icons';
import { doingWords, elapsed, toolWords, useSecondTick } from '../shell/agent-plaque';
import { usePanel } from '../shell/context';
import { flyComet } from '../ui/comet';
import { askToMake, failed } from '../voice/creation';
import { useAgentPlace } from '../voice/drafts';
import { useVoiceAction } from '../voice/phrases';
import { DraftWidgetPreview, madeByVoice, trustOwnDraft, trustedCode } from './draft-preview';
import { DraftCardPreview } from '../cards/draft-card';
import { tr } from '../i18n';
import { byMode } from '../words';

/** Черновики, к которым комета уже летала: второй раз — без полёта, контур на месте. */
const flown = new Set<string>();
/** Комета летит к черновику, только если его начали недавно (только что попросили), а не открыли стол позже. */
const FRESH_MS = 20_000;
/** Сколько шагов агента видно строками в плитке. */
const ROWS = 3;

/** Лицо черновика словами: «виджет», «навык»… */
export function faceWord(draft: DraftView): string {
    switch (draft.meta.kind) {
        case 'game':
            return tr.drafts.faceGame;
        case 'theme':
            return tr.drafts.faceTheme;
        case 'skill':
            return tr.drafts.faceSkill;
        default:
            return tr.drafts.faceWidget;
    }
}

/** Ход работы агента над черновиком, пока он работает: шаги — строками. */
function useWork(draft: DraftView): { steps: AgentStep[]; since: number } {
    const working = draft.meta.stage === 'working';
    const [steps, setSteps] = useState<AgentStep[]>([]);
    const [since, setSince] = useState(draft.meta.updated);
    useEffect(() => {
        if (!working) return;
        let alive = true;
        const read = () =>
            api
                .draftActivity(draft.id)
                .then((log) => {
                    if (!alive) return;
                    setSteps(log.steps);
                    if (log.job) setSince(log.job.since);
                })
                .catch(() => undefined);
        read();
        const timer = window.setInterval(read, 2000);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, [draft.id, working]);
    return { steps, since };
}

/** Пунктирный контур плитки: агент рисует его, пока работает (бегущий штрих), готовое — тихий пунктир. */
function Outline({ drawing, working }: { drawing: boolean; working: boolean }) {
    const id = useRef(`draft-mask-${Math.random().toString(36).slice(2)}`).current;
    return (
        <svg class={`draft-outline${working ? ' is-working' : ''}${drawing ? ' is-drawing' : ''}`} aria-hidden="true">
            <defs>
                <mask id={id}>
                    <rect class="draft-outline-draw" x="0" y="0" width="100%" height="100%" rx="16" pathLength="1000" />
                </mask>
            </defs>
            <rect class="draft-outline-line" mask={`url(#${id})`} x="0" y="0" width="100%" height="100%" rx="16" />
            {working && <rect class="draft-outline-runner" x="0" y="0" width="100%" height="100%" rx="16" pathLength="1000" />}
        </svg>
    );
}

/** Внутри готовой плитки — пример: виджет черновика на примере данных; черновик колонки — после вопроса. */
function Example({ draft }: { draft: DraftView }) {
    const [code, setCode] = useState<string | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const voice = madeByVoice(draft.id) || draft.meta.by === 'voice';
    const hasPanel = draft.files.some((file) => file.path === 'panel.js');
    useEffect(() => {
        setCode(null);
        setProblem(null);
        if (!hasPanel) return;
        // Код черновика работает в своём окошке, без прав пульта (docs/plans/bus.md, «Окошки»): показ не переспрашивает,
        // даже если черновик попросили у колонки.
        trustedCode(draft.id)
            .then((known) => known ?? trustOwnDraft(draft.id))
            .then(setCode)
            .catch((e) => setProblem(problemOf(e)));
    }, [draft.id, draft.version, draft.meta.updated, hasPanel]);
    // Без окошка — карточка, если она есть (её рисует пульт), иначе фразы навыка.
    if (!hasPanel) return <DraftCardPreview id={draft.id} nonce={draft.meta.updated} trying={{ example: true }} fallback={<Phrases draft={draft} />} />;
    if (problem) return <p class="w-cap m-0">{problem}</p>;
    if (!code) return <p class="w-cap m-0">{voice ? byMode(tr.drafts.askedSpeakerLoading, tr.drafts.askedVoiceLoading) : tr.drafts.loadingExample}</p>;
    return <DraftWidgetPreview id={draft.id} nonce={draft.meta.updated} code={code} trying={{ example: true }} bare />;
}

/** Навык и игра: фразы для колонки — что он слышит. */
function Phrases({ draft }: { draft: DraftView }) {
    const [phrases, setPhrases] = useState<string[] | null>(null);
    useEffect(() => {
        api.readDraftFile(draft.id, 'manifest.json')
            .then((file) => {
                const manifest = JSON.parse(file.content);
                const intents: Array<{ phrases?: string[] }> = manifest?.contributes?.intents ?? [];
                setPhrases(intents.flatMap((intent) => intent.phrases ?? []).slice(0, 3));
            })
            .catch(() => setPhrases([]));
    }, [draft.id, draft.meta.updated]);
    if (phrases === null) return <p class="w-cap m-0">{tr.drafts.reading}</p>;
    if (phrases.length === 0) return <p class="w-cap m-0">{draft.meta.kind === 'theme' ? tr.drafts.themeExample : tr.drafts.noPhrases}</p>;
    return (
        <div class="draft-rows">
            {phrases.map((phrase) => (
                <div key={phrase} class="draft-row">
                    «{phrase}»
                </div>
            ))}
        </div>
    );
}

/** Плитка-черновик на месте «Новый виджет»: в работе, на примерке, не вышло, остановлен. `more` — «и ещё N». */
export function DraftTile({ draft, more }: { draft: DraftView; more: number }) {
    const panel = usePanel();
    const box = useRef<HTMLElement>(null);
    const cloud = useAgentPlace();
    const stage = draft.meta.stage;
    const working = stage === 'working';
    const { steps, since } = useWork(draft);
    const now = useSecondTick(working);
    const [drawing, setDrawing] = useState(() => !flown.has(draft.id) && working && Date.now() - draft.meta.updated < FRESH_MS);
    const [redo, setRedo] = useState(false);
    const [words, setWords] = useState('');
    const [busy, setBusy] = useState<string | null>(null);
    const [trouble, setTrouble] = useState<string | null>(null);
    const title = draft.title?.trim() || tr.drafts.newThing;
    const word = faceWord(draft);

    // Только что попросили: комета от «Мозга» к плитке, потом контур рисуется.
    useEffect(() => {
        if (!drawing || !box.current || flown.has(draft.id)) return;
        flown.add(draft.id);
        void flyComet(box.current).then(() => window.setTimeout(() => setDrawing(false), 1200));
    }, [draft.id, drawing]);

    const fit = () => panel.go('fitting', draft.id);
    const put = () => panel.go('fitting', draft.id, undefined, { yes: '' });
    const act = async (name: string, run: () => Promise<unknown>) => {
        setBusy(name);
        setTrouble(null);
        try {
            await run();
        } catch (e) {
            setTrouble(failed(e));
        } finally {
            setBusy(null);
        }
    };
    const again = (said: string) => {
        const text = said.trim();
        if (!text) return setRedo(true);
        void act('redo', () => askToMake(text, cloud, draft.id)).then(() => {
            setRedo(false);
            setWords('');
        });
    };
    const ready = stage === 'ready';
    // Что на кнопках — то и говорите.
    useVoiceAction(ready ? ['примерить', 'покажи', 'примерь'] : [], fit);
    useVoiceAction(ready ? ['на стол', 'поставь', 'включить'] : [], put);
    useVoiceAction(stage !== 'working' ? ['переделай', 'переделать'] : [], (rest) => again(rest));
    useVoiceAction(stage !== 'working' ? ['в черновики'] : [], () => void act('park', () => api.parkDraft(draft.id)));

    // Плитка поднялась на стенд «Примерки»: её место на столе ждёт пунктиром.
    if (panel.link.section === 'fitting' && panel.link.tab === draft.id)
        return (
            <div class="draft-slot" aria-label={tr.drafts.slotLabel(title)}>
                <span class="w-cap">{tr.drafts.slotCap}</span>
            </div>
        );

    const rows = steps.filter((step) => step.kind === 'tool').slice(-ROWS);
    const last = steps[steps.length - 1];
    const asked = draft.meta.by === 'voice' ? byMode(tr.drafts.askedSpeaker, tr.drafts.askedVoice) : null;

    return (
        <div class={`draft-place is-${stage}`}>
            {ready && (
                <span class="draft-flag">
                    <span class="draft-flag-dot" aria-hidden="true" />
                    {tr.drafts.onFitting}
                </span>
            )}
            <section ref={box} class={`draft-tile is-${stage}${drawing ? ' is-drawing' : ''}`} aria-label={tr.drafts.faceLabel(word, title)} aria-busy={working}>
                <Outline drawing={drawing} working={working} />
                <header class="draft-head">
                    <IconSparkle size={16} />
                    <span class={`draft-title${draft.title ? '' : ' is-wait'}`}>{title}</span>
                    <span class="widget-src">{asked ?? word}</span>
                </header>
                {working && (
                    <div class="draft-rows">
                        {rows.length === 0 && <span class="draft-skeleton" aria-hidden="true" />}
                        {rows.map((step) => (
                            <div key={step.seq} class="draft-row is-new">
                                <span class="draft-row-box" aria-hidden="true">
                                    <IconCheck size={10} />
                                </span>
                                {toolWords(step.tool, step.text)}
                            </div>
                        ))}
                    </div>
                )}
                {ready && (
                    <div class="draft-example">
                        <Example draft={draft} />
                    </div>
                )}
                {(stage === 'failed' || stage === 'stopped') && (
                    <p class="draft-end">{stage === 'failed' ? tr.drafts.failedWithNote(draft.meta.note ?? tr.drafts.failedNoteDefault) : tr.drafts.stoppedNote}</p>
                )}
                <footer class="draft-foot">
                    {working ? (
                        <>
                            <span class="agent-spin" aria-hidden="true" />
                            <span class="draft-doing">{last && (last.kind === 'asked' || last.kind === 'result') ? tr.drafts.thinking : doingWords(last, now)}</span>
                            <span class="draft-time">{elapsed(now - since)}</span>
                        </>
                    ) : ready ? (
                        <>
                            <span class="draft-foot-dot" aria-hidden="true" />
                            {tr.drafts.sampleData}
                        </>
                    ) : (
                        <span>{stage === 'failed' ? tr.drafts.failedShort : tr.drafts.stoppedShort}</span>
                    )}
                    {more > 0 && <span class="draft-more">{tr.drafts.andMore(more)}</span>}
                </footer>
            </section>
            {ready && (
                <div class="draft-acts">
                    <button class="pill" onClick={fit}>
                        {tr.drafts.tryOn}
                    </button>
                    <button class="pill pill-own" onClick={put}>
                        {draft.meta.kind === 'widget' ? tr.drafts.toDesk : tr.drafts.enable}
                    </button>
                    <span class="w-cap">{tr.drafts.orSayIt}</span>
                </div>
            )}
            {(stage === 'failed' || stage === 'stopped') && (
                <div class="draft-acts">
                    <button class="pill" disabled={busy !== null} onClick={() => setRedo(!redo)}>
                        {tr.drafts.redoEllipsis}
                    </button>
                    <button class="pill" disabled={busy !== null} onClick={() => void act('park', () => api.parkDraft(draft.id))}>
                        {tr.drafts.toDrafts}
                    </button>
                </div>
            )}
            {redo && (
                <form
                    class="draft-redo"
                    onSubmit={(event) => {
                        event.preventDefault();
                        again(words || tr.drafts.redoDefault);
                    }}
                >
                    <input
                        class="field"
                        value={words}
                        placeholder={tr.drafts.redoPlaceholder}
                        aria-label={tr.drafts.redoLabel}
                        disabled={busy !== null}
                        onInput={(event) => setWords((event.target as HTMLInputElement).value)}
                    />
                    <button type="submit" class="w-btn" disabled={busy !== null}>
                        {busy === 'redo' ? tr.drafts.handing : tr.drafts.redo}
                    </button>
                </form>
            )}
            {trouble && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {trouble}
                </p>
            )}
        </div>
    );
}

/** «Новый виджет» — пунктирная плитка: открывает «+». Рядом с черновиком — меньше: она сдвигается дальше. */
export function AddTile({ onAdd, small }: { onAdd: () => void; small?: boolean }) {
    useVoiceAction(['новый виджет'], onAdd);
    return (
        <button class={`desk-add-tile${small ? ' is-small' : ''}`} onClick={onAdd}>
            <span class="flex items-center gap-6">
                <IconPlus size={16} />
                {tr.drafts.newWidget}
            </span>
            {!small && <span class="w-cap">{tr.drafts.newWidgetCap}</span>}
        </button>
    );
}
