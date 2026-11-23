// Годится ли приложение на витрину сайта (веб-хозяин, docs/plans/web-host.md): по манифесту и коду. На витрине нет шлюза —
// ни модели, ни голоса колонки, ни служб шлюза (сеть, папки, задачи, расписания, умный дом), ни связей с другими
// приложениями. Есть: своё хранилище, свои настройки по умолчанию, язык, окошко (panel.js) с деталями пульта, звук и
// MIDI-клавиатура через браузер. Приложение, которому нужно хоть что-то из первого, на витрину не идёт.
//
// Голосовые фразы (`intents`) и инструменты для модели (`tools`) — входы, которыми DuoWave зовёт приложение снаружи: на
// витрине до них никто не дойдёт, приложение без них живёт. Проверка их не запрещает, а называет — «только в DuoWave».
// Файл чистый: без DOM и Node, его зовут скрипт переноса (scripts/web-tools.ts) и проверки (scripts/web-host-check.ts).

/** Что пакет приносит: тексты его файлов и имена всех файлов в папке. */
export interface PackageFiles {
    manifest: string;
    backend?: string;
    panel?: string;
    names: string[];
}

export interface Checked {
    ok: boolean;
    id: string;
    /** Почему не годится — каждое словами; пусто — годится. */
    problems: string[];
    /** Что есть у приложения, но на витрине не работает и не мешает: голосовые фразы, инструменты модели. */
    only: string[];
}

/** Права, без которых витрина обходится: клавиатура — через Web MIDI браузера. */
const RIGHTS = new Set(['listen:device/midi']);

/** Что приложение может приносить в манифесте: окошко пульта и входы снаружи (их витрина просто не зовёт). */
const CONTRIBUTES = new Set(['panel', 'intents', 'tools']);

/** Что в `contributes.panel` витрина рисует: окошко (разделы и вклады в слоты) и функции, которые оно зовёт. */
const PANEL = new Set(['sections', 'slots', 'calls']);

/** Чем бэкенд может пользоваться у `host`: хранилище, настройки, язык, журнал, кто он. */
const HOST = new Set(['kv', 'settings', 'i18n', 'log', 'plugin', 'api']);

/** Детали SDK пульта, которые веб-хозяин даёт по-настоящему. Остальное — шлюза: папки, медиа, «Поделиться», задачи. */
export const WEB_SDK = new Set([
    'definePlugin',
    'useApi',
    'useSettings',
    'usePanelState',
    'icons',
    'can',
    'Card',
    'Action',
    'Switch',
    'Sheet',
    'Note',
    'Pill',
    'ask',
    'useMidi',
    'useSynth',
    'PianoKeys',
    'readMidiFile',
    'melodyOf',
    'noteName',
    'useLocale',
    'useT',
    't',
    'pick',
]);

/** Модули, которые окошко может импортировать: их даёт import map рамки. */
const MODULES = new Set(['preact', 'preact/hooks', 'preact/jsx-runtime', '@duowave/panel-sdk']);

/** Двери мимо хозяина: сеть, другие скрипты, код из строки. Ни бэкенду, ни окошку их не нужно. */
const DOORS: Array<[RegExp, string]> = [
    [/\bfetch\s*\(/, 'fetch()'],
    [/\bXMLHttpRequest\b/, 'XMLHttpRequest'],
    [/\bWebSocket\b/, 'WebSocket'],
    [/\bEventSource\b/, 'EventSource'],
    [/\bnavigator\s*\.\s*sendBeacon\b/, 'sendBeacon'],
    [/\bimportScripts\b/, 'importScripts'],
    [/\bimport\s*\(/, 'import()'],
    [/\brequire\s*\(/, 'require()'],
    [/\beval\s*\(/, 'eval()'],
    [/\bnew\s+Function\b/, 'new Function'],
];

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);

/**
 * Код без комментариев и (`strings: false`) без содержимого строк: слова в них — не вызовы. Из шаблонных строк
 * остаётся код их `${…}`.
 */
export function codeOnly(text: string, strings = false): string {
    let out = '';
    let i = 0;
    while (i < text.length) {
        const c = text[i];
        const next = text[i + 1];
        if (c === '/' && next === '/') {
            while (i < text.length && text[i] !== '\n') i += 1;
        } else if (c === '/' && next === '*') {
            const end = text.indexOf('*/', i + 2);
            i = end < 0 ? text.length : end + 2;
        } else if (c === '/' && /(^|[(,=:[!&|?{};+\-*%<>~^]|\breturn|\btypeof)\s*$/.test(out.slice(-12))) {
            // Регулярное выражение: кавычки в нём — не строки.
            const from = i;
            let inClass = false;
            i += 1;
            while (i < text.length && text[i] !== '\n' && (text[i] !== '/' || inClass)) {
                if (text[i] === '\\') i += 1;
                else if (text[i] === '[') inClass = true;
                else if (text[i] === ']') inClass = false;
                i += 1;
            }
            i += 1;
            out += strings ? text.slice(from, i) : '/r/';
        } else if (c === '`') {
            // Шаблонная строка: слова — не код, а `${…}` — код, его читаем.
            const from = i;
            let inner = '';
            i += 1;
            while (i < text.length && text[i] !== '`') {
                if (text[i] === '\\') i += 2;
                else if (text[i] === '$' && text[i + 1] === '{') {
                    let depth = 1;
                    const start = i + 2;
                    i += 2;
                    while (i < text.length && depth > 0) {
                        if (text[i] === '{') depth += 1;
                        else if (text[i] === '}') depth -= 1;
                        i += 1;
                    }
                    inner += ` ${codeOnly(text.slice(start, i - 1), strings)} `;
                } else i += 1;
            }
            i += 1;
            out += strings ? text.slice(from, i) : `\`\`${inner}`;
        } else if (c === '"' || c === "'") {
            const from = i;
            i += 1;
            while (i < text.length && text[i] !== c && text[i] !== '\n') i += text[i] === '\\' ? 2 : 1;
            i += 1;
            out += strings ? text.slice(from, i) : `${c}${c}`;
        } else {
            out += c;
            i += 1;
        }
    }
    return out;
}

/** Имена, что окошко берёт у модуля: `import { a, b as c } from "m"` → { m: ['a', 'b'] }; `import x from` — 'default'. */
export function importsOf(text: string): Map<string, string[]> {
    const found = new Map<string, string[]>();
    const pattern = /\bimport\s+([\s\S]*?)\s*from\s*["']([^"']+)["']|\bimport\s*["']([^"']+)["']/g;
    for (const match of text.matchAll(pattern)) {
        const module = match[2] ?? match[3];
        const names: string[] = [];
        const clause = match[1] ?? '';
        const braces = /\{([\s\S]*)\}/.exec(clause);
        if (braces) {
            for (const part of braces[1].split(',')) {
                const name = part.trim().split(/\s+as\s+/)[0]?.trim();
                if (name) names.push(name);
            }
        }
        const outside = clause.replace(/\{[\s\S]*\}/, '').replace(/,/g, ' ').trim();
        if (outside.startsWith('*')) names.push('*');
        else if (outside) names.push('default');
        found.set(module, [...(found.get(module) ?? []), ...names]);
    }
    return found;
}

/** Функции, которые бэкенд отдаёт наружу: `export function x`, `export async function x`, `export const x`. */
export function exportsOf(text: string): string[] {
    return [...text.matchAll(/\bexport\s+(?:async\s+)?(?:function\s*\*?\s*|const\s+|let\s+)([A-Za-z_$][\w$]*)/g)].map((match) => match[1]);
}

/** Проверить пакет: годится ли он на витрину и что в нём работает только в DuoWave. */
export function checkPackage(files: PackageFiles): Checked {
    const problems: string[] = [];
    const only: string[] = [];
    let manifest: Record<string, unknown>;
    try {
        const parsed: unknown = JSON.parse(files.manifest);
        if (!isRecord(parsed)) throw new Error('не объект');
        manifest = parsed;
    } catch (error) {
        return { ok: false, id: '', problems: [`manifest.json не читается: ${error instanceof Error ? error.message : String(error)}`], only };
    }
    const id = typeof manifest.id === 'string' ? manifest.id : '';
    if (!/^[a-z][a-z0-9-]{0,31}$/.test(id)) problems.push(`id «${id}»: строчные латинские буквы, цифры и «-», с буквы`);
    if (manifest.kind !== 'plugin') problems.push(`вид «${String(manifest.kind)}»: на витрину идёт только приложение (kind: plugin)`);

    const rights = Array.isArray(manifest.permissions) ? manifest.permissions.map(String) : [];
    for (const right of rights) if (!RIGHTS.has(right)) problems.push(`просит право «${right}»: на витрине его некому дать`);

    const contributes = isRecord(manifest.contributes) ? manifest.contributes : {};
    for (const key of Object.keys(contributes)) {
        if (!CONTRIBUTES.has(key)) problems.push(`contributes.${key}: это работа шлюза, на витрине её нет`);
    }
    const intents = Array.isArray(contributes.intents) ? contributes.intents.length : 0;
    const tools = Array.isArray(contributes.tools) ? contributes.tools.length : 0;
    if (intents > 0) only.push(`голосовые фразы (${intents})`);
    if (tools > 0) only.push(`инструменты для модели (${tools})`);

    const panel = isRecord(contributes.panel) ? contributes.panel : null;
    if (!panel) problems.push('нет contributes.panel: витрине нечего показать');
    for (const key of Object.keys(panel ?? {})) {
        if (!PANEL.has(key)) problems.push(`contributes.panel.${key}: витрина рисует только окошко (panel.js) — карточкам нужен отрисовщик`);
    }
    const sections = Array.isArray(panel?.sections) ? panel.sections : [];
    if (panel && sections.length === 0) problems.push('у окошка нет раздела (contributes.panel.sections): витрине нечего открыть');
    if (!files.names.includes('panel.js') || files.panel === undefined) problems.push('нет panel.js: на витрине показывают окошко');

    const calls = Array.isArray(panel?.calls) ? panel.calls.map(String) : [];
    if (calls.length > 0 && files.backend === undefined) problems.push('contributes.panel.calls есть, а backend.js нет');
    if (files.backend !== undefined) {
        const exported = new Set(exportsOf(files.backend));
        for (const call of calls) if (!exported.has(call)) problems.push(`функции «${call}» из contributes.panel.calls нет в backend.js`);
        const code = codeOnly(files.backend);
        const used = new Set([...code.matchAll(/\bhost\s*\.\s*([A-Za-z_$][\w$]*)/g)].map((match) => match[1]));
        for (const name of used) if (!HOST.has(name)) problems.push(`backend.js зовёт host.${name}: на витрине этого нет`);
        if (/\bhost\s*\[/.test(code)) problems.push('backend.js берёт у host по имени из переменной (host[…]): проверить нельзя');
        for (const [pattern, word] of DOORS) if (pattern.test(code)) problems.push(`backend.js: ${word} — мимо хозяина`);
        if (/(^|[\n;])\s*import\s*[\w{*"'`]/.test(code)) problems.push('backend.js импортирует модули: бэкенд — один файл');
    }
    for (const name of files.names) {
        if (!['manifest.json', 'backend.js', 'panel.js', 'panel.css', 'README.md'].includes(name)) {
            problems.push(`файл «${name}»: на витрину идут manifest.json, backend.js, panel.js и panel.css`);
        }
    }

    if (files.panel !== undefined) {
        const code = codeOnly(files.panel);
        for (const [module, names] of importsOf(codeOnly(files.panel, true))) {
            if (!MODULES.has(module)) {
                problems.push(`panel.js импортирует «${module}»: окошку дают только preact и @duowave/panel-sdk`);
                continue;
            }
            if (module !== '@duowave/panel-sdk') continue;
            for (const name of names) {
                if (name === '*' || name === 'default') problems.push('panel.js берёт SDK целиком: проверить, чем он пользуется, нельзя');
                else if (!WEB_SDK.has(name)) problems.push(`panel.js берёт «${name}» из SDK: это шлюза, на витрине его нет`);
            }
        }
        for (const [pattern, word] of DOORS) if (pattern.test(code)) problems.push(`panel.js: ${word} — мимо хозяина`);
    }
    return { ok: problems.length === 0, id, problems, only };
}

/** Раздел, который витрина открывает: первый из манифеста. */
export function mainSection(manifest: Record<string, unknown>): string | null {
    const contributes = isRecord(manifest.contributes) ? manifest.contributes : {};
    const panel = isRecord(contributes.panel) ? contributes.panel : {};
    const sections = Array.isArray(panel.sections) ? panel.sections : [];
    const first = sections[0];
    return isRecord(first) && typeof first.id === 'string' ? first.id : null;
}
