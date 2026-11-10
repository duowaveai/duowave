// Редактор кода Мастерской с подсветкой (docs/design/roles: макеты HelperWorkshop, WindowWorkshop): ключевые слова —
// бирюзовый `#0F766E`, функции — синий `#1D4ED8`, строки — малиновый `#9F1239`, комментарии — серый курсив `#78716C`.
// Лайм, янтарь и фиолет не берутся: в данных они значат «кто отвечает». Поле ввода — прозрачное поверх раскрашенного
// текста, прокрутка общая; очень большой файл остаётся без подсветки.

import { useLayoutEffect, useRef } from 'preact/hooks';

const KEYWORDS =
    'const|let|var|function|return|if|else|for|while|do|switch|case|break|continue|import|export|from|default|async|await|new|class|extends|try|catch|finally|throw|typeof|instanceof|in|of|null|true|false|undefined|this|yield|delete|void';

const JS = new RegExp(
    `(\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/)|("(?:\\\\.|[^"\\\\\\n])*"|'(?:\\\\.|[^'\\\\\\n])*'|\`(?:\\\\.|[^\`\\\\])*\`)|\\b(${KEYWORDS})\\b|([A-Za-z_$][\\w$]*)(?=\\s*\\()`,
    'g',
);
const JSON_TOKENS = /("(?:\\.|[^"\\\n])*")|\b(true|false|null)\b/g;
const CSS_TOKENS = /(\/\*[\s\S]*?\*\/)|("(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*')|(@[\w-]+|!important)|(var|calc|rgba?|color-mix)(?=\()/g;

/** Больше этого — без подсветки: раскраска на каждой букве стала бы заметна. */
const LIMIT = 200_000;

const escape = (text: string): string => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** HTML раскрашенного текста: классы hl-c (комментарий), hl-s (строка), hl-k (ключевое слово), hl-f (функция). */
export function highlight(text: string, language: string): string {
    if (text.length > LIMIT) return escape(text);
    const pattern = language === 'js' ? JS : language === 'json' ? JSON_TOKENS : language === 'css' ? CSS_TOKENS : null;
    if (!pattern) return escape(text);
    pattern.lastIndex = 0;
    let html = '';
    let at = 0;
    for (const found of text.matchAll(pattern)) {
        const start = found.index ?? 0;
        html += escape(text.slice(at, start));
        const [token] = found;
        const kind =
            language === 'json'
                ? found[1]
                    ? 'hl-s'
                    : 'hl-k'
                : found[1]
                  ? 'hl-c'
                  : found[2]
                    ? 'hl-s'
                    : found[3]
                      ? 'hl-k'
                      : 'hl-f';
        html += `<span class="${kind}">${escape(token)}</span>`;
        at = start + token.length;
    }
    return html + escape(text.slice(at));
}

/** Язык файла по его имени: js, json, css или без подсветки. */
export function languageOf(path: string): string {
    if (/\.(m?js|ts)$/.test(path)) return 'js';
    if (path.endsWith('.json')) return 'json';
    if (path.endsWith('.css')) return 'css';
    return 'text';
}

export function CodeEditor({
    value,
    path,
    rows,
    onInput,
    onKeyDown,
}: {
    value: string;
    path: string;
    rows: number;
    onInput: (value: string) => void;
    onKeyDown?: (event: KeyboardEvent) => void;
}) {
    const input = useRef<HTMLTextAreaElement>(null);
    const shown = useRef<HTMLPreElement>(null);
    const sync = () => {
        if (!input.current || !shown.current) return;
        shown.current.scrollTop = input.current.scrollTop;
        shown.current.scrollLeft = input.current.scrollLeft;
    };
    // Новый текст (файл открыли, агент поправил) — раскраска встаёт туда же, куда прокручено поле.
    useLayoutEffect(sync, [value]);
    return (
        <div class="code-editor">
            <pre ref={shown} class="code-hl font-mono ws-editor" aria-hidden="true">
                {/* Последний перевод строки — чтобы раскраска не была на строку короче поля. */}
                <code dangerouslySetInnerHTML={{ __html: `${highlight(value, languageOf(path))}\n` }} />
            </pre>
            <textarea
                ref={input}
                class="field font-mono ws-editor code-input"
                rows={rows}
                spellcheck={false}
                autocapitalize="off"
                autocomplete="off"
                aria-label={`Файл ${path}`}
                value={value}
                onInput={(event) => onInput((event.target as HTMLTextAreaElement).value)}
                onScroll={sync}
                onKeyDown={(event) => onKeyDown?.(event as KeyboardEvent)}
            />
        </div>
    );
}
