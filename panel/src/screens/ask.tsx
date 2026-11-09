import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, AskEvent, askBrain, problemOf } from '../api';
import { IconSparkle } from '../icons';
import { Sheet } from '../ui/sheet';

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
                    setMeta(`${event.model || 'модель'}${event.provider ? ` · ${event.provider}` : ''} · инструментов MCP: ${event.tools}`);
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
                setLines((all) => [...all, { kind: 'note', text: e instanceof ApiError ? problemOf(e) : 'Шлюз недоступен — попробуйте ещё раз.', error: true }]);
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
            title="Спросить мозг"
            about={meta ?? 'та же модель и те же инструменты, что у колонки — без колонки'}
            icon={
                <span class="dash-circle dash-sheet-icon" aria-hidden="true">
                    <IconSparkle size={20} />
                </span>
            }
            onClose={onClose}
            footer={
                <form
                    class="flex gap-8 items-center"
                    onSubmit={(e) => {
                        e.preventDefault();
                        void send();
                    }}
                >
                    <input
                        class="field flex-1 min-w-0"
                        aria-label="Вопрос"
                        placeholder="Например: что у нас на сегодня в Todoist?"
                        value={text}
                        onInput={(e) => setText((e.target as HTMLInputElement).value)}
                    />
                    <button class="pill pill-own dash-confirm-btn" type="submit" disabled={busy || !text.trim()}>
                        {busy ? '…' : 'Спросить'}
                    </button>
                    {lines.length > 0 && (
                        <button class="pill pill-off dash-confirm-btn" type="button" onClick={restart}>
                            Заново
                        </button>
                    )}
                </form>
            }
        >
            <div role="log" aria-live="polite" class="flex flex-col gap-8 max-h-52vh-460 overflow-y-auto">
                {!lines.length && (
                    <p class="t-caption m-0">
                        Спросите то, что спросили бы колонку: модель ответит словами и, если нужно, вызовет инструменты подключённых
                        серверов. Команды самой колонке здесь не выполняются.
                    </p>
                )}
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
                                {line.ms !== undefined && <span class="t-caption">первое слово за {(line.ms / 1000).toFixed(1).replace('.', ',')} с</span>}
                            </div>
                        );
                    }
                    if (line.kind === 'tool') {
                        return (
                            <div key={index} class="flex flex-col gap-4 py-8 px-12 r-16 border-glass">
                                <span class="flex gap-6 items-center flex-wrap">
                                    <b class="fs-14">{toolTitle(line.name)}</b>
                                    {line.server && <span class="pill pill-off">{line.server}</span>}
                                    {line.result?.outside && <span class="pill pill-yandex">текст из интернета</span>}
                                    {!line.result && <span class="spinner spinner-sm" aria-label="ждём ответ сервера" />}
                                </span>
                                <span class="t-mono opacity-80">
                                    {JSON.stringify(line.input)}
                                </span>
                                {line.result && (
                                    <span class={`t-mono ${line.result.isError ? 'c-dot-offline' : 'c-sub'}`}>
                                        {line.result.content}
                                    </span>
                                )}
                            </div>
                        );
                    }
                    return (
                        <p key={index} role={line.error ? 'alert' : 'status'} class={`t-caption m-0${line.error ? ' c-dot-offline' : ''}`}>
                            {line.text}
                        </p>
                    );
                })}
                <div ref={end} />
            </div>
        </Sheet>
    );
}
