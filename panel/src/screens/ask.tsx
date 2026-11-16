import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, AskEvent, askBrain, problemOf } from '../api';
import { IconSparkle } from '../icons';
import { formatNumber, tr } from '../i18n';
import { Sheet } from '../ui/sheet';
import { byMode } from '../words';

/** Одна реплика разговора: вопрос, ответ модели или шаг инструмента. */
type Line =
    | { kind: 'question'; text: string }
    | { kind: 'answer'; text: string; ms?: number }
    | { kind: 'tool'; name: string; server?: string; input: Record<string, unknown>; result?: { content: string; isError: boolean; outside: boolean } }
    | { kind: 'note'; text: string; error?: boolean };

/** «mcp_home__HassTurnOn» → «HassTurnOn». */
const toolTitle = (name: string): string => name.split('__').pop() ?? name;

/**
 * «Спросить мозг»: проверочный чат с той же моделью и теми же инструментами MCP, что у колонки, — без колонки.
 * Видно, какой инструмент модель вызвала, с чем, и что он вернул: по этому подсказку серверу правят прицельно.
 */
export function AskSheet({ onClose }: { onClose: () => void }) {
    const [lines, setLines] = useState<Line[]>([]);
    const [text, setText] = useState('');
    const [busy, setBusy] = useState(false);
    const [meta, setMeta] = useState<string | null>(null);
    const fresh = useRef(true);
    const abort = useRef<AbortController | null>(null);
    const end = useRef<HTMLDivElement>(null);

    useEffect(() => () => abort.current?.abort(), []);
    useEffect(() => end.current?.scrollIntoView({ block: 'end' }), [lines]);

    const apply = (event: AskEvent): void => {
        setLines((all) => {
            const next = [...all];
            const last = next[next.length - 1];
            switch (event.type) {
                case 'start':
                    setMeta(tr.ask.meta(event.model || tr.ask.metaModel, event.provider ?? '', event.tools));
                    return next;
                case 'text':
                    if (last?.kind === 'answer') next[next.length - 1] = { ...last, text: `${last.text} ${event.text}`.trim(), ms: last.ms ?? event.ms };
                    else next.push({ kind: 'answer', text: event.text, ms: event.ms });
                    return next;
                case 'tool':
                    next.push({ kind: 'tool', name: event.name, server: event.server, input: event.input });
                    return next;
                case 'result': {
                    const index = next.map((line) => line.kind === 'tool' && line.name === event.name && !line.result).lastIndexOf(true);
                    if (index >= 0) {
                        const line = next[index] as Extract<Line, { kind: 'tool' }>;
                        next[index] = { ...line, result: { content: event.content, isError: event.isError, outside: event.outside } };
                    }
                    return next;
                }
                case 'error':
                    next.push({ kind: 'note', text: event.message, error: true });
                    return next;
                default:
                    return next;
            }
        });
    };

    const send = async () => {
        const question = text.trim();
        if (!question || busy) return;
        setText('');
        setBusy(true);
        setLines((all) => [...all, { kind: 'question', text: question }]);
        const controller = new AbortController();
        abort.current = controller;
        try {
            await askBrain(question, fresh.current, apply, controller.signal);
            fresh.current = false;
        } catch (e) {
            if (!controller.signal.aborted) {
                setLines((all) => [...all, { kind: 'note', text: e instanceof ApiError ? problemOf(e) : tr.ask.unreachable, error: true }]);
            }
        } finally {
            setBusy(false);
        }
    };

    const restart = () => {
        abort.current?.abort();
        fresh.current = true;
        setLines([]);
        setMeta(null);
    };

    return (
        <Sheet
            title={tr.ask.title}
            about={meta ?? byMode(tr.ask.aboutStation, tr.ask.aboutComputer)}
            icon={
                <span class="dash-circle dash-sheet-icon" aria-hidden="true">
                    <IconSparkle size={20} />
                </span>
            }
            onClose={onClose}
            footer={
                <form
                    class="flex gap-8 items-center w-full"
                    onSubmit={(e) => {
                        e.preventDefault();
                        void send();
                    }}
                >
                    <input
                        class="field flex-1 min-w-0"
                        aria-label={tr.ask.questionLabel}
                        placeholder={tr.ask.placeholder}
                        value={text}
                        onInput={(e) => setText((e.target as HTMLInputElement).value)}
                    />
                    {lines.length > 0 && (
                        <button class="btn btn-plain" type="button" onClick={restart}>
                            {tr.ask.restart}
                        </button>
                    )}
                    <button class="btn btn-main" type="submit" disabled={busy || !text.trim()} aria-busy={busy || undefined}>
                        {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                        {tr.ask.ask}
                    </button>
                </form>
            }
        >
            <div role="log" aria-live="polite" class="flex flex-col gap-8">
                {!lines.length && <p class="win-lead">{byMode(tr.ask.introStation, tr.ask.introComputer)}</p>}
                {lines.map((line, index) => {
                    if (line.kind === 'question') {
                        return (
                            <p key={index} class="m-0 self-end max-w-85p py-8 px-12 r-16 bg-icon-bg">
                                {line.text}
                            </p>
                        );
                    }
                    if (line.kind === 'answer') {
                        return (
                            <div key={index} class="flex flex-col gap-2 max-w-92p">
                                <p class="m-0">{line.text}</p>
                                {line.ms !== undefined && <span class="win-hint">{tr.ask.firstWord(formatNumber(line.ms / 1000, { minimumFractionDigits: 1, maximumFractionDigits: 1 }))}</span>}
                            </div>
                        );
                    }
                    if (line.kind === 'tool') {
                        return (
                            <div key={index} class="win-group">
                                <div class="win-row is-col">
                                    <span class="flex gap-6 items-center flex-wrap">
                                        <span class="win-code">{toolTitle(line.name)}</span>
                                        {line.server && <span class="win-chip">{line.server}</span>}
                                        {line.result?.outside && <span class="pill pill-yandex">{tr.ask.outsideText}</span>}
                                        {!line.result && <span class="spinner spinner-sm" aria-label={tr.ask.waitingServer} />}
                                    </span>
                                    <span class="t-mono opacity-80">{JSON.stringify(line.input)}</span>
                                    {line.result && <span class={`t-mono ${line.result.isError ? 'c-dot-offline' : 'c-sub'}`}>{line.result.content}</span>}
                                </div>
                            </div>
                        );
                    }
                    return (
                        <p key={index} role={line.error ? 'alert' : 'status'} class={line.error ? 'win-callout m-0' : 'win-note'}>
                            {line.text}
                        </p>
                    );
                })}
                <div ref={end} />
            </div>
        </Sheet>
    );
}
