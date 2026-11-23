// Веб-хозяин (docs/plans/web-host.md): страница, которая на сайте DuoWave стоит вместо шлюза и пульта для одного
// приложения с витрины. Окошко приложения — та же изолированная рамка, что в пульте (frame.html, `sandbox allow-scripts`),
// с тем же panel.js; его backend.js — в воркере (worker.ts) с `host` в браузере (backend.ts); клавиатура — Web MIDI
// (midi.ts). Чего на витрине нет, окошко узнаёт ответом-ошибкой словами.
//
// Адрес: index.html?app=<id>&lang=en|ru[&mode=dark]. Открывается главный раздел приложения — его окно. Приложения — рядом, в ../apps/<id>/, их список —
// ../apps/index.json (его пишет скрипт переноса, scripts/web-tools.ts). Странице сайта хозяин говорит о себе
// сообщениями `duowave-tool:*`: высоту окна, «готово», «не открылось».

import type { FrameState, FromFrame, ToFrame } from '../frame/protocol';
import { ours } from '../frame/protocol';
import { DEFAULT_THEME, activeCss } from '../shell/themes';
import type { MidiEvent } from '../sdk/midi';
import { Json, defaultSettings, languagesOf } from './backend';
import { MIDI_WORDS, WebMidi } from './midi';
import type { FromWorker, ToWorker } from './worker';

/** Запись списка витрины: что приложение и что хозяину о нём знать. */
export interface ShelfEntry {
    id: string;
    version: string;
    /** Раздел, который открывается (первый из манифеста). */
    section: string;
    styles: boolean;
}

/** Хозяин → страница сайта. */
export type ToSite =
    | { type: 'duowave-tool:size'; height: number }
    | { type: 'duowave-tool:ready'; app: string }
    | { type: 'duowave-tool:failed'; app: string; message: string };

const WORDS = {
    en: {
        notHere: 'Not in the web version: this works in DuoWave.',
        failed: 'The app did not open:',
        frame: (title: string) => `${title} — app window`,
        unknown: 'There is no such app here.',
    },
    ru: {
        notHere: 'В веб-версии этого нет: это работает в DuoWave.',
        failed: 'Приложение не открылось:',
        frame: (title: string) => `${title} — окно приложения`,
        unknown: 'Такого приложения здесь нет.',
    },
};

const params = new URLSearchParams(location.search);
const appId = params.get('app') ?? '';
const lang: 'en' | 'ru' = params.get('lang') === 'ru' ? 'ru' : 'en';
const mode: 'light' | 'dark' = params.get('mode') === 'dark' ? 'dark' : 'light';
const words = WORDS[lang];
const STORE = `duowave.tool.${appId}`;

function toSite(message: ToSite): void {
    if (window.parent !== window) window.parent.postMessage(message, location.origin);
}

function fail(message: string): void {
    const box = document.getElementById('root');
    if (box) {
        const line = document.createElement('p');
        line.className = 'web-host-failed';
        line.textContent = `${words.failed} ${message}`;
        box.replaceChildren(line);
    }
    toSite({ type: 'duowave-tool:failed', app: appId, message });
    reportHeight();
}

function reportHeight(): void {
    toSite({ type: 'duowave-tool:size', height: Math.ceil(document.documentElement.getBoundingClientRect().height) });
}

function readStore(): Record<string, Json> {
    try {
        const saved = JSON.parse(localStorage.getItem(STORE) ?? '{}') as unknown;
        return saved && typeof saved === 'object' && !Array.isArray(saved) ? (saved as Record<string, Json>) : {};
    } catch {
        return {};
    }
}

function writeStore(store: Record<string, Json>): void {
    try {
        localStorage.setItem(STORE, JSON.stringify(store));
    } catch {
        // Память браузера закрыта: приложение живёт до перезагрузки.
    }
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);

/** Название приложения на языке страницы: `locales.<язык>.title`, иначе своё. */
function titleOf(manifest: Record<string, unknown>): string {
    const locales = isRecord(manifest.locales) ? manifest.locales : {};
    const own = isRecord(locales[lang]) ? (locales[lang] as Record<string, unknown>).title : undefined;
    return String(own ?? manifest.title ?? appId);
}

async function start(): Promise<void> {
    if (!/^[a-z][a-z0-9-]{0,31}$/.test(appId)) return fail(words.unknown);
    const apps = new URL('../apps/', location.href);
    const shelf = (await fetch(new URL('index.json', apps)).then((answer) => answer.json())) as ShelfEntry[];
    const entry = shelf.find((item) => item.id === appId);
    if (!entry) return fail(words.unknown);
    const base = new URL(`${appId}/`, apps);
    const manifest = (await fetch(new URL('manifest.json', base)).then((answer) => answer.json())) as Record<string, unknown>;
    const contributes = isRecord(manifest.contributes) ? manifest.contributes : {};
    const panel = isRecord(contributes.panel) ? contributes.panel : {};
    const calls = Array.isArray(panel.calls) ? panel.calls.map(String) : [];
    const settings = defaultSettings(manifest);
    const title = titleOf(manifest);

    // ——— Бэкенд в воркере ———
    const store = readStore();
    const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
    const waiting = new Map<number, (message: Extract<FromWorker, { type: 'answer' }>) => void>();
    let workerSeq = 0;
    let version = 0;
    const backendReady = new Promise<void>((resolve, reject) => {
        worker.addEventListener('message', (event: MessageEvent<FromWorker>) => {
            const message = event.data;
            if (message.type === 'ready') resolve();
            else if (message.type === 'failed') reject(new Error(message.message));
            else if (message.type === 'log') (message.level === 'error' ? console.error : message.level === 'warn' ? console.warn : console.info)(`[${appId}]`, message.text);
            else if (message.type === 'answer') {
                waiting.get(message.seq)?.(message);
                waiting.delete(message.seq);
            }
        });
        worker.addEventListener('error', (event) => reject(new Error(event.message || 'worker')));
    });
    const init: ToWorker = {
        type: 'init',
        backend: new URL('backend.js', base).href,
        calls,
        setup: { id: appId, version: String(manifest.version ?? '0'), locale: lang, languages: languagesOf(manifest), settings, kv: store },
    };
    worker.postMessage(init);

    /** Вызов функции бэкенда; записал что-то в хранилище — сохранить и сказать окошку «данные поменялись». */
    const call = async (fn: string, input: unknown): Promise<unknown> => {
        await backendReady;
        workerSeq += 1;
        const seq = workerSeq;
        const answer = await new Promise<Extract<FromWorker, { type: 'answer' }>>((resolve) => {
            waiting.set(seq, resolve);
            worker.postMessage({ type: 'call', seq, fn, input } satisfies ToWorker);
        });
        if (answer.writes.length > 0) {
            for (const [key, value] of answer.writes) {
                if (value === undefined) delete store[key];
                else store[key] = value;
            }
            writeStore(store);
            version += 1;
            post({ type: 'duowave:state', state: state() });
        }
        if (!answer.ok) throw new Error(answer.error ?? words.failed);
        return answer.value;
    };

    // ——— Окошко ———
    const state = (): FrameState => ({
        locale: lang,
        app: appId,
        version,
        settings,
        permissions: [],
        station: { device: null, name: '', online: false, mode: null, role: 'gateway' },
        live: { connected: false, presence: {} },
        theme: { css: activeCss(DEFAULT_THEME, mode), attrs: { theme: mode, themeId: DEFAULT_THEME.id, look: 'minimal', shell: 'desk' } },
        draft: false,
    });
    let frame: HTMLIFrameElement | null = null;
    const post = (message: ToFrame): void => frame?.contentWindow?.postMessage(message, '*');
    const module = new URL('panel.js', base).href;
    const styles = entry.styles ? new URL('panel.css', base).href : null;
    let midi: WebMidi | null = null;
    let stopListening: (() => void) | null = null;
    const root = document.getElementById('root')!;

    const open = (part: { kind: 'section'; id: string }): void => {
        midi?.close();
        midi = null;
        stopListening?.();
        frame?.remove();
        const made = document.createElement('iframe');
        made.setAttribute('sandbox', 'allow-scripts');
        made.setAttribute('referrerpolicy', 'no-referrer');
        made.setAttribute('allow', 'midi');
        made.title = words.frame(title);
        made.className = 'web-host-frame';
        made.src = new URL('frame.html', location.href).href;
        frame = made;
        let started = false;
        const answer = (seq: number, work: Promise<unknown>): void => {
            work.then(
                (value) => post({ type: 'duowave:answer', seq, ok: true, value }),
                (error: unknown) => post({ type: 'duowave:answer', seq, ok: false, error: error instanceof Error ? error.message : String(error) }),
            );
        };
        const notHere = (seq: number): void => answer(seq, Promise.reject(new Error(words.notHere)));
        const onMessage = (event: MessageEvent): void => {
            if (event.source !== made.contentWindow || !ours(event.data)) return;
            const message = event.data as FromFrame;
            switch (message.type) {
                case 'duowave:ready':
                    // Окошко загрузилось заново само — второй раз ему ничего не дают (как в пульте).
                    if (started) {
                        made.remove();
                        fail(words.notHere);
                        return;
                    }
                    started = true;
                    post({ type: 'duowave:init', module, styles, part, state: state() });
                    break;
                case 'duowave:size':
                    made.style.height = `${Math.max(120, Math.min(4000, Math.ceil(message.height)))}px`;
                    requestAnimationFrame(reportHeight);
                    break;
                case 'duowave:failed':
                    fail(message.message);
                    break;
                case 'duowave:call':
                    answer(message.seq, call(String(message.fn), message.input ?? {}));
                    break;
                case 'duowave:ask':
                    answer(message.seq, Promise.resolve(window.confirm([message.options?.title, message.options?.text].filter(Boolean).join('\n\n'))));
                    break;
                case 'duowave:go':
                    // Своё окно уже открыто; чужих разделов пульта на витрине нет.
                    break;
                case 'duowave:midi': {
                    if (!midi) {
                        midi = new WebMidi(
                            MIDI_WORDS[lang],
                            (midiState) => post({ type: 'duowave:midi-state', state: midiState }),
                            (midiEvent: MidiEvent) => post({ type: 'duowave:midi-event', event: midiEvent }),
                        );
                    }
                    midi.handle(String(message.op), Array.isArray(message.args) ? message.args : []);
                    break;
                }
                case 'duowave:tasks':
                    if (message.op === 'retain') post({ type: 'duowave:tasks-state', tasks: [] });
                    break;
                case 'duowave:file':
                case 'duowave:media':
                case 'duowave:open-media':
                case 'duowave:share':
                case 'duowave:record':
                case 'duowave:settings-save':
                case 'duowave:midi-file':
                case 'duowave:task-cancel':
                    notHere(message.seq);
                    break;
                case 'duowave:rights':
                    answer(message.seq, Promise.resolve({ asked: [], pending: 0 }));
                    break;
            }
        };
        window.addEventListener('message', onMessage);
        stopListening = () => window.removeEventListener('message', onMessage);
        root.replaceChildren(made);
        made.addEventListener('load', () => toSite({ type: 'duowave-tool:ready', app: appId }), { once: true });
    };

    backendReady.catch((error: unknown) => fail(error instanceof Error ? error.message : String(error)));
    open({ kind: 'section', id: entry.section });
    new ResizeObserver(reportHeight).observe(document.documentElement);
}

document.documentElement.lang = lang;
start().catch((error: unknown) => fail(error instanceof Error ? error.message : String(error)));
