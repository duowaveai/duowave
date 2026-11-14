// SDK пульта внутри окошка приложения (docs/plans/bus.md, «Окошки»): тот же «@duowave/panel-sdk», что у встроенных
// (docs/plugins/panel-sdk.d.ts), но всё, что выходит за окошко, идёт мостом к пульту: функции своего бэкенда,
// картинки, вопрос «да/нет», переход в раздел, MIDI-клавиатура. Свой Preact и детали пульта — те же, в своей теме.

import * as preact from 'preact';
import type { ComponentChildren } from 'preact';
import * as hooks from 'preact/hooks';
import * as jsxRuntime from 'preact/jsx-runtime';

import { IconCoins, IconGlobe, IconHeart, IconList, IconMusic, IconPackage, IconSparkle, IconSpeaker, IconTimer, IconWeather } from '../icons';
import { definePlugin } from '../shell/registry';
import { melodyOf, noteName } from '../sdk/notes';
import type { MidiSong } from '../sdk/notes';
import { PianoKeys } from '../sdk/piano-keys';
import { useSynth } from '../sdk/synth';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import type { FrameMidiState, FrameState, FrameTask, FromFrame } from './protocol';
import { tr } from '../i18n';
import { pickText, translate, useLocale, useT } from '../i18n/app';

// ——— Состояние, что прислал пульт ———

let state: FrameState | null = null;
const watchers = new Set<() => void>();

export function setState(next: FrameState): void {
    state = next;
    for (const watch of [...watchers]) watch();
}

export function currentState(): FrameState | null {
    return state;
}

function useFrameState(): FrameState | null {
    const [, rerender] = hooks.useState(0);
    hooks.useEffect(() => {
        const watch = (): void => rerender((n) => n + 1);
        watchers.add(watch);
        return () => {
            watchers.delete(watch);
        };
    }, []);
    return state;
}

// ——— Мост ———

let seq = 0;
const waiting = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();

export function send(message: FromFrame): void {
    window.parent.postMessage(message, '*');
}

/** Вопрос пульту; ответ — когда он ответит. */
function request<T>(make: (seq: number) => FromFrame): Promise<T> {
    seq += 1;
    const id = seq;
    return new Promise<T>((resolve, reject) => {
        waiting.set(id, { resolve: resolve as (value: unknown) => void, reject });
        send(make(id));
    });
}

export function answered(id: number, ok: boolean, value: unknown, error?: string): void {
    const pending = waiting.get(id);
    if (!pending) return;
    waiting.delete(id);
    if (ok) pending.resolve(value);
    else pending.reject(new Error(error ?? tr.frame.failed));
}

// ——— Хуки и детали SDK ———

/** Бэкенд своего приложения: функции из contributes.panel.calls. Чужое — через `host.bus` в бэкенде. */
function useApi(_pluginId?: string): { call: <T = unknown>(fn: string, input?: unknown) => Promise<T> } {
    return hooks.useMemo(() => ({ call: <T,>(fn: string, input: unknown = {}) => request<T>((id) => ({ type: 'duowave:call', seq: id, fn, input })) }), []);
}

function useSettings(_pluginId?: string): Record<string, unknown> {
    const now = useFrameState();
    return hooks.useMemo(() => ({ ...(now?.settings ?? {}) }), [now?.settings]);
}

/** Картинка своего приложения: пульт берёт её с ключом входа и отдаёт окошку из памяти. */
/** A picture of the home folder by its address (`files/home/Photos/лето.jpg`), made small: the app's link to that
 *  folder and the person's own rights both hold. */
function useFolderFile(address: string, options: { size?: number } = {}): string | null {
    const [url, setUrl] = hooks.useState<string | null>(null);
    const version = useFrameState()?.version ?? 0;
    const size = options.size ?? 960;
    hooks.useEffect(() => {
        if (!address) return;
        let alive = true;
        let made: string | null = null;
        request<Blob>((id) => ({ type: 'duowave:file', seq: id, name: '', address, size }))
            .then((blob) => {
                if (!alive) return;
                made = URL.createObjectURL(blob);
                setUrl(made);
            })
            .catch(() => alive && setUrl(null));
        return () => {
            alive = false;
            if (made) URL.revokeObjectURL(made);
        };
    }, [address, size, version]);
    return url;
}

function useFile(name: string): string | null {
    const [url, setUrl] = hooks.useState<string | null>(null);
    const version = useFrameState()?.version ?? 0;
    hooks.useEffect(() => {
        if (!name) return;
        let alive = true;
        let made: string | null = null;
        request<Blob>((id) => ({ type: 'duowave:file', seq: id, name }))
            .then((blob) => {
                if (!alive) return;
                made = URL.createObjectURL(blob);
                setUrl(made);
            })
            .catch(() => alive && setUrl(null));
        return () => {
            alive = false;
            if (made) URL.revokeObjectURL(made);
        };
    }, [name, version]);
    return url;
}

/**
 * Видео или звук своей папки (`files/app/<id>/…`) — адрес для `<video src>` / `<audio src>`: шлюз отдаёт файл с диска с
 * перемоткой. Пока адреса нет (или файла нет) — null.
 */
function useMedia(name: string | null | undefined): string | null {
    const [url, setUrl] = hooks.useState<string | null>(null);
    hooks.useEffect(() => {
        if (!name) {
            setUrl(null);
            return;
        }
        let alive = true;
        mediaUrl(name)
            .then((made) => alive && setUrl(made))
            .catch(() => alive && setUrl(null));
        return () => {
            alive = false;
        };
    }, [name]);
    return url;
}

/** То же без хука — для списка серий и смены ролика в плеере. */
function mediaUrl(name: string): Promise<string> {
    return request<string>((id) => ({ type: 'duowave:media', seq: id, name }));
}

type MediaWhat = string | { path: string; title?: string };

/**
 * Свой файл (или несколько) — в просмотрщике пульта, во всё окно пульта: фото листать, видео с перемоткой и во весь
 * экран, звук. Пути — своей папки (`episodes/01.mp4`) или папок дома по связи приложения (`files/home/Photos/…`);
 * `{ path, title }` — со своим названием в шапке. Ответ — когда человек закрыл: на какой вещи и на какой секунде
 * (продолжить свой плеер оттуда).
 */
function openMedia(what: MediaWhat | MediaWhat[], options: { start?: number; at?: number } = {}): Promise<{ index: number; at: number | null }> {
    const items = (Array.isArray(what) ? what : [what]).map((item) =>
        typeof item === 'string' ? { name: item } : { name: String(item?.path ?? ''), title: item?.title ? String(item.title) : undefined },
    );
    return request((id) => ({ type: 'duowave:open-media', seq: id, items, options }));
}

/**
 * «Поделиться» файлом своей папки и текстом к посту: пульт показывает своё окно — системное меню телефона, «Сохранить в
 * «Видео»», «Скачать», «Скопировать текст», страницы площадок. Ответ — куда ушло (null — человек закрыл окно).
 */
function share(what: { file?: string; text?: string; title?: string }): Promise<{ where: string | null; saved?: string | null }> {
    return request<{ where: string | null; saved?: string | null }>((id) => ({ type: 'duowave:share', seq: id, request: what }));
}

/** Связи, что приложение просит в манифесте, и разрешены ли они: `{ asked: [{ right, words, allowed }], pending }`. */
function rights(): Promise<{ asked: Array<{ right: string; words: string; allowed: boolean }>; pending: number }> {
    return request((id) => ({ type: 'duowave:rights', seq: id, op: 'state' }));
}

/** Все «да» одним листом пульта: что приложение просит, словами, и «Разрешить всё». Ответ — связи после этого. */
function askRights(): Promise<{ asked: Array<{ right: string; words: string; allowed: boolean }>; pending: number }> {
    return request((id) => ({ type: 'duowave:rights', seq: id, op: 'ask' }));
}

/** Свои настройки из окна (ключ сервиса, канал): пульт пишет их, как окно настроек приложения; меняет администратор. */
function saveSettings(values: Record<string, unknown>): Promise<boolean> {
    return request((id) => ({ type: 'duowave:settings-save', seq: id, values }));
}

/** Запись своим голосом: пульт открывает микрофон и окно записи; ответ — файл записи в своей папке и его длина. */
function record(options: { title?: string; text?: string; maxSeconds?: number } = {}): Promise<{ file: string; seconds: number }> {
    return request<{ file: string; seconds: number }>((id) => ({ type: 'duowave:record', seq: id, options }));
}

function usePanelState(): { version: number; go: (section: string, tab?: string, item?: string) => void } {
    const now = useFrameState();
    return { version: now?.version ?? 0, go: (section, tab, item) => send({ type: 'duowave:go', section, tab, item }) };
}

/** Живой канал — только открыт ли он и как слышно колонки: запросы людей окошку не видны. */
function useLive(): { connected: boolean; requests: unknown[]; presence: Record<string, unknown> } {
    const now = useFrameState();
    return { connected: now?.live.connected ?? false, requests: [], presence: now?.live.presence ?? {} };
}

function useStation(): FrameState['station'] {
    const now = useFrameState();
    return now?.station ?? { device: null, name: '', online: false, mode: null, role: 'gateway' };
}

function can(permission: string): boolean {
    return state?.permissions.includes(permission) ?? false;
}

/** «Да/нет» спрашивает пульт поверх всего, с именем приложения. */
function ask(options: { title: string; text?: string; ok?: string; danger?: boolean }): Promise<boolean> {
    return request<boolean>((id) => ({ type: 'duowave:ask', seq: id, options }));
}

export const icons = {
    weather: IconWeather,
    music: IconMusic,
    timer: IconTimer,
    list: IconList,
    coins: IconCoins,
    globe: IconGlobe,
    heart: IconHeart,
    speaker: IconSpeaker,
    sparkle: IconSparkle,
    package: IconPackage,
};

/** Имя значка, которым приложение его взяло; чужой — «package». */
export function iconName(icon: unknown): string {
    for (const [name, value] of Object.entries(icons)) if (value === icon) return name;
    return 'package';
}

function Card({ title, meta, children }: { title?: string; meta?: string; children?: ComponentChildren }) {
    return (
        <section class="card glass card-list">
            {(title || meta) && (
                <div class="card-head">
                    {title && <h2 class="t-title-xs m-0">{title}</h2>}
                    {meta && <span class="t-meta">{meta}</span>}
                </div>
            )}
            {children}
        </section>
    );
}

function Note({ tone = 'meta', children }: { tone?: 'meta' | 'warning' | 'error'; children?: ComponentChildren }) {
    const cls = tone === 'error' ? 'c-dot-offline' : tone === 'warning' ? 'c-pill-yandex-fg' : '';
    return <p class={`t-caption m-0 ${cls}`}>{children}</p>;
}

function Pill({ tone = 'off', children }: { tone?: 'own' | 'yandex' | 'agent' | 'off'; children?: ComponentChildren }) {
    return <span class={`pill pill-${tone}`}>{children}</span>;
}

// ——— MIDI-клавиатура через мост: та же, что у пульта, одна на страницу ———

let midiState: FrameMidiState = {
    state: 'connecting',
    via: 'gateway',
    why: null,
    inputs: [],
    outputs: [],
    input: null,
    output: null,
    delayMs: null,
    browserPossible: false,
};
const midiWatchers = new Set<() => void>();
const midiListeners = new Set<(event: unknown) => void>();
let midiRefs = 0;

export function setMidiState(next: FrameMidiState): void {
    midiState = next;
    for (const watch of [...midiWatchers]) watch();
}

export function midiEvent(event: unknown): void {
    for (const listener of [...midiListeners]) {
        try {
            listener(event);
        } catch (error) {
            console.error('MIDI:', error);
        }
    }
}

const midi = (op: string, ...args: unknown[]): void => send({ type: 'duowave:midi', op, args });

function useMidi() {
    const [, rerender] = hooks.useState(0);
    hooks.useEffect(() => {
        const watch = (): void => rerender((n) => n + 1);
        midiWatchers.add(watch);
        midiRefs += 1;
        if (midiRefs === 1) midi('retain');
        return () => {
            midiWatchers.delete(watch);
            midiRefs -= 1;
            if (midiRefs === 0) midi('release');
        };
    }, []);
    return hooks.useMemo(
        () => ({
            ...midiState,
            choose: (id: string) => midi('choose', id),
            on: (handler: (event: unknown) => void) => {
                midiListeners.add(handler);
                return () => {
                    midiListeners.delete(handler);
                };
            },
            press: (note: number, velocity?: number) => midi('press', note, velocity),
            release: (note: number) => midi('release-note', note),
            send: (event: unknown) => midi('send', event),
            play: (notes: Array<[number, number, number, number]>, options?: { id?: string }) => midi('play', notes, options?.id),
            stop: () => midi('stop'),
            useBrowser: async () => midi('browser'),
        }),
        [midiState],
    );
}

// ——— Долгие задачи своего приложения через мост: ход и отмена (gateway/src/tasks) ———

let tasks: FrameTask[] = [];
/** Пульт прислал задачи хоть раз. */
let tasksReady = false;
const taskWatchers = new Set<() => void>();
let taskRefs = 0;

export function setTasks(next: FrameTask[]): void {
    tasks = next;
    tasksReady = true;
    for (const watch of [...taskWatchers]) watch();
}

/**
 * Долгая задача своего приложения: её начинает бэкенд (Host API, `POST /api/v1/apps/host/tasks`) и возвращает id из
 * своей функции — а окошко видит ход и просит отменить. Без id — задача, что идёт сейчас, а нет такой — последняя.
 */
function useTask(id?: string): { task: FrameTask | null; tasks: FrameTask[]; ready: boolean; cancel: () => Promise<void> } {
    const [, rerender] = hooks.useState(0);
    hooks.useEffect(() => {
        const watch = (): void => rerender((n) => n + 1);
        taskWatchers.add(watch);
        taskRefs += 1;
        if (taskRefs === 1) send({ type: 'duowave:tasks', op: 'retain' });
        return () => {
            taskWatchers.delete(watch);
            taskRefs -= 1;
            if (taskRefs === 0) send({ type: 'duowave:tasks', op: 'release' });
        };
    }, []);
    const task = (id ? tasks.find((entry) => entry.id === id) : tasks[0]) ?? null;
    const taskId = task?.id;
    const cancel = hooks.useCallback(
        () => (taskId ? request<void>((seq) => ({ type: 'duowave:task-cancel', seq, id: taskId })) : Promise.reject(new Error(tr.frame.noTask))),
        [taskId],
    );
    return { task, tasks, ready: tasksReady, cancel };
}

/** Файл `.mid` читает шлюз — через пульт. */
async function readMidiFile(file: Blob): Promise<MidiSong> {
    if (file.size > 256 * 1024) throw new Error(tr.frame.midiTooBig);
    return request<MidiSong>((id) => ({ type: 'duowave:midi-file', seq: id, file }));
}

export const sdk = {
    definePlugin,
    useApi,
    useSettings,
    useFile,
    useMedia,
    mediaUrl,
    openMedia,
    share,
    record,
    rights,
    askRights,
    saveSettings,
    useFolderFile,
    usePanelState,
    useLive,
    useStation,
    icons,
    can,
    Card,
    Action,
    Switch,
    Sheet,
    Note,
    Pill,
    ask,
    useMidi,
    useTask,
    useSynth,
    PianoKeys,
    readMidiFile,
    melodyOf,
    noteName,
    // Языки (docs/plans/i18n.md): язык пульта и тексты приложения по нему.
    useLocale,
    useT,
    t: translate,
    pick: pickText,
};

/** Отдать модулю приложения Preact и SDK окошка — до его загрузки (import map ведёт к public/sdk/*). */
export function installFrameSdk(): void {
    (globalThis as unknown as { __duowave: unknown }).__duowave = { preact, hooks, jsxRuntime, sdk };
}
