// `host` для backend.js на витрине сайта — та же форма, что даёт шлюз (gateway/src/plugins/quickjs.rs, PRELUDE), но
// живёт в браузере: хранилище — в памяти страницы (хозяин кладёт его в localStorage), настройки — значения по умолчанию
// из манифеста, язык — язык страницы. Всего, чего на витрине нет (сеть, шина, папки, задачи), у этого `host` тоже нет:
// зов отвечает ошибкой словами. Проверка (check.ts) не пускает на витрину приложения, которым это нужно.
// Файл чистый: без DOM, его зовут воркер (worker.ts) и проверки (scripts/web-host-check.ts).

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** Что о себе знает приложение на витрине. */
export interface BackendSetup {
    id: string;
    version: string;
    /** Язык страницы: `en`, `ru`. */
    locale: string;
    /** Языки приложения из манифеста (`languages`); первым — его родной. */
    languages: string[];
    settings: Record<string, Json>;
    /** Хранилище, каким его сохранил хозяин. */
    kv: Record<string, Json>;
}

/** Что изменил вызов в хранилище: ключ и новое значение (`undefined` — стёрт). */
export type Writes = Array<[string, Json | undefined]>;

const NOT_HERE = 'На витрине сайта этого нет: оно работает в DuoWave.';

/** Настройки по умолчанию из манифеста: `settings: [{ key, default }]`. */
export function defaultSettings(manifest: Record<string, unknown>): Record<string, Json> {
    const out: Record<string, Json> = {};
    const list = Array.isArray(manifest.settings) ? manifest.settings : [];
    for (const item of list) {
        if (item && typeof item === 'object' && typeof (item as { key?: unknown }).key === 'string') {
            const { key, default: value } = item as { key: string; default?: Json };
            if (value !== undefined) out[key] = value;
        }
    }
    return out;
}

/** Языки приложения: `languages` манифеста, без него — русский (как у шлюза). */
export function languagesOf(manifest: Record<string, unknown>): string[] {
    const list = Array.isArray(manifest.languages) ? manifest.languages.filter((code): code is string => typeof code === 'string') : [];
    return list.length ? list : ['ru'];
}

/** Форма числа, как у шлюза: QuickJS без Intl, формы знакомых языков — здесь. */
export function pluralOf(code: string, count: unknown): string {
    const n = Math.abs(Number(count));
    if (!Number.isFinite(n)) return 'other';
    const whole = Number.isInteger(n);
    if (['ru', 'uk', 'be'].includes(code)) {
        if (!whole) return 'other';
        const units = n % 10;
        const tens = n % 100;
        if (units === 1 && tens !== 11) return 'one';
        if (units >= 2 && units <= 4 && (tens < 12 || tens > 14)) return 'few';
        return 'many';
    }
    if (code === 'pl') {
        if (!whole) return 'other';
        const units = n % 10;
        const tens = n % 100;
        if (n === 1) return 'one';
        if (units >= 2 && units <= 4 && (tens < 12 || tens > 14)) return 'few';
        return 'many';
    }
    if (['fr', 'pt'].includes(code)) return n < 2 ? 'one' : 'other';
    if (['ja', 'zh', 'ko', 'vi', 'th', 'id', 'ms'].includes(code)) return 'other';
    return n === 1 ? 'one' : 'other';
}

const clone = <T>(value: T): T => (value === undefined ? value : (JSON.parse(JSON.stringify(value)) as T));

/** `host` приложения и журнал записей в хранилище, которые хозяин сохранит. */
export function makeHost(setup: BackendSetup, log: (level: string, text: string) => void = () => {}) {
    const store = new Map<string, Json>(Object.entries(setup.kv));
    const writes: Writes = [];
    const say =
        (level: string) =>
        (...parts: unknown[]): void =>
            log(level, parts.map((part) => (typeof part === 'string' ? part : JSON.stringify(part))).join(' '));
    const order = (): string[] => [...new Set([setup.locale, 'en', setup.languages[0], 'ru'])];
    const pick = (variants: unknown): unknown => {
        if (variants === null || typeof variants !== 'object') return variants;
        const map = variants as Record<string, unknown>;
        for (const code of order()) if (map[code] !== undefined) return map[code];
        return Object.values(map)[0];
    };
    const t = (messages: Record<string, Record<string, unknown>> | undefined, key: string, params: Record<string, unknown> = {}): string => {
        for (const code of order()) {
            let text = messages?.[code]?.[key];
            if (text === undefined) continue;
            if (text !== null && typeof text === 'object') {
                const forms = text as Record<string, unknown>;
                text = forms[pluralOf(code, params?.count)] ?? forms.other ?? forms.many ?? forms.one ?? '';
            }
            return String(text).replace(/\{(\w+)\}/g, (whole, name: string) => (params && name in params ? String(params[name]) : whole));
        }
        return String(key);
    };
    const missing = (what: string) => () => {
        throw new Error(`host.${what}: ${NOT_HERE}`);
    };
    const nowhere = (what: string, names: string[]) => Object.freeze(Object.fromEntries(names.map((name) => [name, missing(`${what}.${name}`)])));
    const host = Object.freeze({
        api: 2,
        plugin: Object.freeze({ id: setup.id, version: setup.version }),
        kv: Object.freeze({
            get: (key: unknown) => clone(store.get(String(key)) ?? null),
            set: (key: unknown, value: unknown) => {
                const stored = clone((value === undefined ? null : value) as Json);
                store.set(String(key), stored);
                writes.push([String(key), stored]);
            },
            delete: (key: unknown) => {
                store.delete(String(key));
                writes.push([String(key), undefined]);
            },
            keys: (prefix: unknown = '') => [...store.keys()].filter((key) => key.startsWith(String(prefix ?? ''))).sort(),
        }),
        settings: Object.freeze({
            get: (key?: string) => (key === undefined ? clone(setup.settings) : clone(setup.settings[key])),
        }),
        log: say('info'),
        i18n: Object.freeze({
            get locale() {
                return setup.locale;
            },
            languages: Object.freeze([...setup.languages]),
            pick,
            t,
            plural: (count: unknown, forms: Record<string, string> | undefined) => {
                const form = pluralOf(setup.locale, count);
                return forms?.[form] ?? forms?.other ?? forms?.many ?? forms?.one ?? '';
            },
        }),
        http: nowhere('http', ['fetch']),
        bus: nowhere('bus', ['call', 'tell', 'list', 'ask']),
        files: nowhere('files', ['list', 'find', 'text', 'preview', 'play', 'copy', 'read', 'write', 'remove', 'move', 'mkdir', 'stat', 'download']),
        tasks: nowhere('tasks', ['start', 'progress', 'done', 'fail', 'get']),
    });
    const console = Object.freeze({ log: say('info'), info: say('info'), debug: say('info'), warn: say('warn'), error: say('error') });
    /** Записи с прошлого раза — и забыть их. */
    const taken = (): Writes => writes.splice(0, writes.length);
    return { host, console, taken };
}

/** Вызов функции бэкенда, как его делает шлюз для окошка: только из `contributes.panel.calls`, ответ — как JSON. */
export async function callBackend(
    module: Record<string, unknown>,
    calls: string[],
    fn: string,
    input: unknown,
): Promise<Json> {
    if (!calls.includes(fn)) throw new Error(`«${fn}» нет в contributes.panel.calls`);
    const exported = module[fn];
    if (typeof exported !== 'function') throw new Error(`backend.js не отдаёт «${fn}»`);
    const value: unknown = await (exported as (input: unknown) => unknown)(clone(input ?? {}));
    return value === undefined ? null : clone(value as Json);
}
