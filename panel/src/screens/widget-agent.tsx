// «Новый виджет: голосом или текстом» (docs/plans/desk.md, этап 5; макет SpaceAddPanel): просьба уходит агенту
// мастерской с заготовкой виджета (черновик `widget-…`), готовый черновик показывается живым предпросмотром — его
// panel.js с шлюза по адресу-токену, данные — из песочницы мастерской, — «Поставить на стол» ставит плагин (права — с
// «да» администратора) и ставит виджет на стол; «Иначе…» — поправить словами. Голосом — распознавание браузера, если
// оно есть; а колонке можно сказать «сделай виджет…». Пока агент рисует — его шаги («пишет panel.js», «проверяет») и
// сколько идёт работа, с «Остановить»; панель можно закрыть — работа идёт на шлюзе, её видно на плашке «Делаю виджет».
//
// panel.js черновика выполняется в самом пульте, с правами вошедшего. В «+» просьбу человек только что написал сам; в
// мастерской «Показать виджет» сначала спрашивает (mayRunDraft): у черновика, сделанного голосом у колонки, — каждый
// раз, у прочих — когда код не тот, что уже разрешали. Показ выполняет только разрешённый текст panel.js.

import { useEffect, useRef, useState } from 'preact/hooks';

import { AgentStep, InstallPlan, api, can, problemOf, workshopChat } from '../api';
import { IconMic, IconSparkle } from '../icons';
import { DraftPreview } from '../sdk';
import { aboutOf, doingWords, elapsed, toolWords, useSecondTick } from '../shell/agent-plaque';
import type { SlotContribution } from '../shell/slot';
import type { DeskWidget } from '../shell/widgets';
import { ask } from '../ui/confirm';
import { WidgetFrame } from '../ui/widget';
import { gw } from '../via';

/**
 * Чей panel.js человек уже разрешил выполнить в пульте — и какой: разрешение держится за текст кода, и другой код
 * спрашивает снова. До перезагрузки страницы. Черновики, сделанные голосом, сюда не попадают: их каждый показ — вопрос.
 */
const trusted = new Map<string, string>();

/** Черновик сделан голосом у колонки (gateway/src/plugins/workshop/voice.rs: `golos-…`): попросить мог кто угодно. */
const madeByVoice = (id: string): boolean => id.startsWith('golos-');

/** panel.js черновика, как он лежит сейчас. */
const panelCode = (id: string): Promise<string> => api.readDraftFile(id, 'panel.js').then((answer) => answer.content);

/**
 * Можно ли выполнить panel.js черновика в пульте: код работает с правами того, кто вошёл. Ответ — тот текст panel.js,
 * что человек разрешил (показ выполнит только его), или null. Черновик, сделанный голосом (`golos-…`), спрашивает каждый
 * раз, с прямым предупреждением: попросить его мог кто угодно рядом с колонкой, а его агент правит код и потом. Прочие
 * помнят разрешённый код и спрашивают снова, когда он стал другим.
 */
export async function mayRunDraft(id: string): Promise<string | null> {
    const code = await panelCode(id);
    const voice = madeByVoice(id);
    const before = trusted.get(id);
    if (!voice && before === code) return code;
    trusted.delete(id);
    const yes = await ask({
        title: voice ? 'Показать виджет, сделанный голосом?' : 'Показать виджет черновика?',
        text: [
            voice ? 'Этот черновик сделан голосом у колонки: попросить его мог кто угодно рядом с ней — даже звук из телевизора.' : null,
            !voice && before !== undefined ? 'panel.js поменялся с тех пор, как вы его показывали.' : null,
            'panel.js черновика выполнится в пульте с вашими правами: всё, что можете вы, сможет и он.',
            voice ? 'Сначала прочтите его panel.js в «Файлах». Вопрос будет перед каждым показом.' : null,
        ]
            .filter(Boolean)
            .join(' '),
        ok: 'Показать',
        danger: voice,
    });
    if (!yes) return null;
    if (!voice) trusted.set(id, code);
    return code;
}

/** Стили черновика на странице, пока его предпросмотр открыт. */
function addStyles(href: string): HTMLLinkElement {
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = gw(href);
    document.head.appendChild(link);
    return link;
}

/** Название из просьбы: без «сделай виджет», до 40 знаков, с заглавной. */
function titleOf(request: string): string {
    const text = request
        .replace(/^\s*(сделай|нарисуй|создай|добавь)\s+(мне\s+)?(новый\s+)?виджет\s*[:,—-]?\s*/i, '')
        .replace(/[«»"]/g, '')
        .trim();
    const short = text.length > 40 ? `${text.slice(0, 38).trim()}…` : text;
    return short ? short[0].toUpperCase() + short.slice(1) : 'Новый виджет';
}

const INSTRUCTIONS =
    'Это виджет для стола пульта. Что он показывает — в panel.js (слот desk.widgets; рамку, название и меню даёт стол, ' +
    'рисуй только содержимое, классы w-big, w-rows > w-row, w-cap, w-day); данные — функция panelData в backend.js, ' +
    'её зовёт panel.js через useApi. panelData только читает: в host.kv она не пишет, разве что кэш по сроку (когда ' +
    'прежний устарел), — каждая запись в kv перечитывает виджеты, и иначе они перечитываются раз в секунду. ' +
    'Нужен интернет — добавь в манифест право net:<хост>. Название виджета в title. ' +
    'Сделай, проверь (check) и коротко скажи, что получилось.';

/** Распознавание речи браузера, если оно есть (Chrome, Safari); иначе — нет кнопки микрофона. */
function useDictation(onText: (text: string) => void): { supported: boolean; listening: boolean; start: () => void } {
    const [listening, setListening] = useState(false);
    const Recognition =
        typeof window === 'undefined'
            ? undefined
            : ((window as unknown as Record<string, unknown>).SpeechRecognition ?? (window as unknown as Record<string, unknown>).webkitSpeechRecognition);
    const start = () => {
        if (!Recognition || listening) return;
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const recognition = new (Recognition as any)();
        recognition.lang = 'ru-RU';
        recognition.interimResults = false;
        recognition.onresult = (event: { results: ArrayLike<ArrayLike<{ transcript: string }>> }) => {
            const said = event.results[0]?.[0]?.transcript;
            if (said) onText(said);
        };
        recognition.onend = () => setListening(false);
        recognition.onerror = () => setListening(false);
        setListening(true);
        recognition.start();
    };
    return { supported: !!Recognition, listening, start };
}

interface Preview {
    id: string;
    contribution: SlotContribution;
}

/** Вызов агента, пока он рисует: что делает и чем кончилось. */
interface LiveCall {
    key: number;
    /** Инструмент: по нему ответ находит свой вызов. */
    tool: string;
    words: string;
    /** undefined — ещё идёт. */
    ok?: boolean;
}

/** Сколько последних вызовов видно, пока агент рисует. */
const LIVE_CALLS = 5;

export function WidgetAgent({ onPut }: { onPut: (key: string) => void }) {
    const [request, setRequest] = useState('');
    const [fix, setFix] = useState('');
    const [fixing, setFixing] = useState(false);
    const [draft, setDraft] = useState<string | null>(null);
    const [said, setSaid] = useState('');
    const [busy, setBusy] = useState<'draw' | 'fix' | 'put' | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [note, setNote] = useState<string | null>(null);
    const [preview, setPreview] = useState<Preview | null>(null);
    /** Шаги агента в этом разговоре: последний — чем он занят, вызовы — списком. */
    const [last, setLast] = useState<AgentStep | null>(null);
    const [calls, setCalls] = useState<LiveCall[]>([]);
    const [since, setSince] = useState(0);
    /** Черновик, над которым агент работает в этом разговоре: его и останавливает «Остановить». */
    const [agentDraft, setAgentDraft] = useState<string | null>(null);
    const [stopping, setStopping] = useState(false);
    /** «Остановить» нажали: что агент успел, остаётся в черновике, а показывать его нечего. */
    const stopped = useRef(false);
    const styles = useRef<HTMLLinkElement | null>(null);
    /**
     * Панель открыта. Закрыли посреди «Рисую…» — пульт перестаёт слушать и ничего не ставит, а агент доделывает
     * черновик на шлюзе (плашка «Делаю виджет», мастерская).
     */
    const alive = useRef(true);
    const talk = useRef<AbortController | null>(null);
    const dictation = useDictation((text) => (draft ? setFix(text) : setRequest(text)));
    const mayMake = can('plugins.manage');
    const working = busy === 'draw' || busy === 'fix';
    const now = useSecondTick(working);

    // Предпросмотр уходит вместе с панелью: разговор обрывается, стили черновика снимаются.
    useEffect(
        () => () => {
            alive.current = false;
            talk.current?.abort();
            styles.current?.remove();
            styles.current = null;
        },
        [],
    );

    const chat = async (id: string, message: string, fresh: boolean) => {
        setSaid('');
        setCalls([]);
        setLast({ seq: 0, at: Date.now(), kind: 'asked', text: message });
        setSince(Date.now());
        setStopping(false);
        stopped.current = false;
        let failed: string | null = null;
        let count = 0;
        talk.current?.abort();
        const controller = new AbortController();
        talk.current = controller;
        setAgentDraft(id);
        // Агент закончил (или его остановили) — «Остановить» больше не о чем.
        const done = () => alive.current && setAgentDraft(null);
        await workshopChat(
            id,
            message,
            fresh,
            (event) => {
                const at = Date.now();
                if (event.type === 'text') {
                    setSaid((before) => before + event.text);
                    setLast({ seq: 0, at, kind: 'said', text: event.text });
                }
                if (event.type === 'tool') {
                    const about = aboutOf(event.input);
                    const key = (count += 1);
                    setCalls((before) => [...before, { key, tool: event.name, words: toolWords(event.name, about) }].slice(-LIVE_CALLS));
                    setLast({ seq: 0, at, kind: 'tool', tool: event.name, text: about });
                }
                if (event.type === 'result') {
                    // Вызовы одного хода идут разом, и ответы приходят не по порядку; номера вызова разговор не даёт —
                    // ответ достаётся последнему идущему вызову того же инструмента.
                    setCalls((before) => {
                        const open = before.map((call) => call.ok === undefined && call.tool === event.name).lastIndexOf(true);
                        return open < 0 ? before : before.map((call, index) => (index === open ? { ...call, ok: !event.isError } : call));
                    });
                    setLast({ seq: 0, at, kind: 'result', tool: event.name, text: event.content, failed: event.isError });
                }
                if (event.type === 'error') failed = event.message;
            },
            controller.signal,
        ).finally(done);
        if (failed) throw new Error(failed);
    };

    const show = async (id: string) => {
        const { module, styles: css } = await api.draftPreview(id);
        if (!alive.current) return;
        // Просьбу человек написал сам, здесь и сейчас: показ его черновика не переспрашивает. Разрешённым запоминается
        // этот panel.js — мастерская спросит, когда код станет другим.
        trusted.set(id, await panelCode(id));
        if (!alive.current) return;
        styles.current?.remove();
        styles.current = css ? addStyles(css) : null;
        const loaded = await import(/* @vite-ignore */ gw(module));
        if (!alive.current) return;
        const plugin = loaded.default ?? loaded.plugin;
        const contribution: SlotContribution | undefined = plugin?.slots?.['desk.widgets']?.[0];
        if (!contribution) throw new Error('В черновике нет виджета стола (слот desk.widgets) — скажите «Иначе…», что поправить.');
        setPreview({ id, contribution });
    };

    /** Агента остановили: сделанное — в черновике, в мастерской; ошибкой это не считается. */
    const wasStopped = (id: string) => setNote(`Остановлено. Что агент успел, осталось в черновике «${id}» — он в мастерской.`);

    const draw = async () => {
        const text = request.trim();
        if (!text) return;
        setBusy('draw');
        setError(null);
        setNote(null);
        setPreview(null);
        let id: string | null = null;
        try {
            id = `widget-${Date.now().toString(36)}`;
            await api.createDraft({ id, kind: 'widget', title: titleOf(text) });
            setDraft(id);
            await chat(id, `Сделай виджет: «${text}». ${INSTRUCTIONS}`, true);
            if (!alive.current) return;
            if (stopped.current) return wasStopped(id);
            await show(id);
        } catch (e) {
            if (!alive.current) return;
            if (stopped.current && id) wasStopped(id);
            else setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
        } finally {
            if (alive.current) setBusy(null);
        }
    };

    const refine = async () => {
        const text = fix.trim();
        if (!draft || !text) return;
        setBusy('fix');
        setError(null);
        setNote(null);
        try {
            await chat(draft, `${text}. Поправь виджет и проверь (check).`, false);
            if (!alive.current) return;
            if (stopped.current) return wasStopped(draft);
            await show(draft);
            if (!alive.current) return;
            setFix('');
            setFixing(false);
        } catch (e) {
            if (!alive.current) return;
            if (stopped.current) wasStopped(draft);
            else setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
        } finally {
            if (alive.current) setBusy(null);
        }
    };

    /**
     * «Остановить»: работа агента над черновиком кончается на шлюзе, и разговор закрывается сам. Не закрылся за пять
     * секунд — пульт перестаёт его слушать.
     */
    const stop = () => {
        if (!agentDraft) return;
        const listening = talk.current;
        stopped.current = true;
        setStopping(true);
        void api
            .stopDraftAgent(agentDraft)
            .then((answer) => {
                // Останавливать было нечего: агент как раз закончил — показать, что вышло.
                if (!answer.stopped) stopped.current = false;
                else window.setTimeout(() => listening?.abort(), 5000);
            })
            .catch((e) => {
                stopped.current = false;
                if (!alive.current) return;
                setStopping(false);
                setError(problemOf(e));
            });
    };

    const put = async () => {
        if (!draft || !preview) return;
        setBusy('put');
        setError(null);
        try {
            const plan: InstallPlan = await api.installPlan(draft);
            const rights = plan.added.length > 0 ? `Права и хосты: ${plan.added.join(', ')}.` : 'Новых прав нет.';
            const yes = await ask({ title: `Поставить «${plan.title}» на стол?`, text: `Виджет станет плагином ${plan.to}. ${rights}`, ok: 'Поставить' });
            if (!yes) return;
            const report = await api.installDraft(draft, true);
            if (!report.installed) throw new Error(report.message);
            onPut(`package.${draft}.${preview.contribution.id}`);
        } catch (e) {
            setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    if (!mayMake)
        return <p class="w-cap m-0">Новый виджет рисует агент мастерской — это может администратор.</p>;

    const widget: DeskWidget | null = preview
        ? {
              ...preview.contribution,
              plugin: `package.${preview.id}`,
              key: `preview.${preview.id}`,
              size: preview.contribution.size ?? 'small',
              initial: true,
          }
        : null;
    const Component = preview?.contribution.Component;

    return (
        <section class="widget-agent" aria-labelledby="widget-agent-h">
            <h3 id="widget-agent-h" class="add-sub">
                Скажите, что показать, — нарисую
            </h3>
            <form
                class="widget-agent-ask"
                onSubmit={(event) => {
                    event.preventDefault();
                    void draw();
                }}
            >
                <textarea
                    class="field"
                    rows={2}
                    value={request}
                    placeholder="Сделай виджет: сколько осталось стирке и что потом — крупно"
                    aria-label="Что показать на виджете"
                    disabled={busy !== null}
                    onInput={(event) => setRequest((event.target as HTMLTextAreaElement).value)}
                />
                <div class="flex items-center gap-8">
                    {dictation.supported && (
                        <button type="button" class="widget-agent-mic" aria-label="Сказать голосом" aria-pressed={dictation.listening} onClick={dictation.start}>
                            <IconMic size={18} />
                        </button>
                    )}
                    <button type="submit" class="w-btn w-btn-ink" disabled={busy !== null || !request.trim()}>
                        {busy === 'draw' ? 'Рисую…' : 'Нарисовать'}
                    </button>
                    <span class="w-cap">или колонке: «Алиса, сделай виджет…»</span>
                </div>
            </form>
            {working && (
                <div class="agent-live">
                    <div class="agent-live-head">
                        <span class="agent-spin" aria-hidden="true" />
                        <span class="agent-live-doing">
                            {stopping ? 'останавливаю…' : agentDraft ? doingWords(last ?? undefined, now) : 'показываю, что вышло'}
                        </span>
                        <span class="agent-live-time">{elapsed(now - since)}</span>
                        <button type="button" class="w-btn" disabled={stopping || !agentDraft} onClick={stop}>
                            Остановить
                        </button>
                    </div>
                    {calls.length > 0 && (
                        <ol class="agent-live-steps" aria-label="Что агент уже сделал">
                            {calls.map((call) => (
                                <li key={call.key} class={call.ok === false ? 'is-failed' : undefined}>
                                    {call.words}
                                    {call.ok === true ? ' ✓' : call.ok === false ? ' — не вышло' : ''}
                                </li>
                            ))}
                        </ol>
                    )}
                    <p class="w-cap m-0">
                        Панель можно закрыть: агент доделает сам, а чем он занят, покажет плашка «Делаю виджет» — нажмите её, и
                        откроется ход работы в мастерской. Готовый виджет — там же, оттуда он встанет на стол.
                    </p>
                </div>
            )}
            {said && (
                <p class="widget-agent-said">
                    <IconSparkle size={14} />
                    {said}
                </p>
            )}
            {widget && Component && (
                <div class="widget-agent-preview">
                    <DraftPreview.Provider value={preview.id}>
                        <WidgetFrame widget={widget} menu={{ hide: () => setPreview(null) }}>
                            <Component />
                        </WidgetFrame>
                    </DraftPreview.Provider>
                    <div class="flex flex-wrap items-center gap-8">
                        <button class="w-btn w-btn-ink" disabled={busy !== null} onClick={() => void put()}>
                            {busy === 'put' ? 'Ставлю…' : 'Поставить на стол'}
                        </button>
                        <button class="w-btn" disabled={busy !== null} onClick={() => setFixing(!fixing)}>
                            Иначе…
                        </button>
                    </div>
                    <p class="w-cap m-0">потом его можно двигать и растягивать; поправить — в мастерской</p>
                    {fixing && (
                        <form
                            class="widget-agent-ask"
                            onSubmit={(event) => {
                                event.preventDefault();
                                void refine();
                            }}
                        >
                            <input
                                class="field"
                                value={fix}
                                placeholder="Что поменять: крупнее, другой цвет, ещё строка…"
                                aria-label="Что поменять в виджете"
                                disabled={busy !== null}
                                onInput={(event) => setFix((event.target as HTMLInputElement).value)}
                            />
                            <button type="submit" class="w-btn" disabled={busy !== null || !fix.trim()}>
                                {busy === 'fix' ? 'Правлю…' : 'Поправить'}
                            </button>
                        </form>
                    )}
                </div>
            )}
            {note && (
                <p class="w-cap m-0" role="status">
                    {note}
                </p>
            )}
            {error && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}

/**
 * Виджет черновика, как он встанет на стол (мастерская, docs/plans/desk.md, этап 5): его panel.js по адресу-токену,
 * данные — из песочницы. `nonce` — показать заново после правки. `code` — panel.js, который человек разрешил
 * (mayRunDraft): стал другим, пока спрашивали, — код не выполняется.
 */
export function DraftWidgetPreview({ id, nonce, code }: { id: string; nonce: number; code: string }) {
    const [contribution, setContribution] = useState<SlotContribution | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        let alive = true;
        let link: HTMLLinkElement | null = null;
        setError(null);
        setContribution(null);
        void (async () => {
            try {
                const { module, styles: css } = await api.draftPreview(id);
                // Код сейчас — тот, что разрешили? Агент мог переписать его, пока человек отвечал.
                const now = await panelCode(id);
                // Закрыли, пока шлюз отвечал: ни стилей, ни кода.
                if (!alive) return;
                if (now !== code) throw new Error('panel.js поменялся, пока вы решали, — нажмите «Показать заново».');
                if (css) link = addStyles(css);
                const loaded = await import(/* @vite-ignore */ gw(module));
                if (!alive) return;
                const found: SlotContribution | undefined = (loaded.default ?? loaded.plugin)?.slots?.['desk.widgets']?.[0];
                if (!found) throw new Error('В черновике нет виджета стола (слот desk.widgets).');
                setContribution(found);
            } catch (e) {
                if (alive) setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
            }
        })();
        return () => {
            alive = false;
            link?.remove();
        };
    }, [id, nonce, code]);
    if (error) return <p class="t-caption c-dot-offline m-0">{error}</p>;
    if (!contribution) return <p class="t-caption m-0">Загружаю виджет…</p>;
    const { Component } = contribution;
    const widget: DeskWidget = { ...contribution, plugin: `package.${id}`, key: `preview.${id}`, size: contribution.size ?? 'small', initial: true };
    return (
        <div class="widget-agent-preview">
            <DraftPreview.Provider value={id}>
                <WidgetFrame widget={widget} menu={{ hide: () => setContribution(null) }}>
                    <Component />
                </WidgetFrame>
            </DraftPreview.Provider>
        </div>
    );
}
