// Окошко приложения со стороны пульта (docs/plans/bus.md, «Окошки»): изолированный фрейм `sandbox="allow-scripts"` с
// frame.html, мост к нему и разведка — что приложение приносит в пульт (разделы, виджеты), не пуская его код в пульт.
// Чьё окошко, пульт знает по самому фрейму: функции зовутся только у этого приложения (их ещё раз проверяет шлюз —
// contributes.panel.calls), картинки — только его, MIDI — его канал. Темой, размером и настройками окошко живёт по
// словам пульта.

import { FunctionComponent, h } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { api, can, fetchBusPreview, fetchPluginFile } from '../api';
import { usePluginVersion } from '../plugins/versions';
import { useApi, useLive, useSettings, useStation } from '../sdk';
import { channelOf } from '../sdk/midi';
import { cancelTask, tasksOf, watchTasks } from '../tasks/state';
import type { PanelPlugin } from '../shell/registry';
import type { SectionDef } from '../shell/sections';
import type { SlotContribution } from '../shell/slot';
import { useTheme } from '../shell/theme';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { IconPackage } from '../icons';
import { icons } from '../sdk';
import { FramePart, FrameShape, FrameState, FromFrame, ToFrame, ours } from './protocol';
import { language, tr, useLanguage } from '../i18n';

/** Страница окошка — пульта же, не через проход: её код один на всех. */
const FRAME_PAGE = '/panel/frame.html';
/** Разведка ждёт ответа не дольше. */
const PROBE_MS = 15_000;
/** Окошко не выше — дальше прокрутка внутри. */
const MAX_HEIGHT = 4000;

function themeNow(): FrameState['theme'] {
    const attrs: Record<string, string> = {};
    for (const [key, value] of Object.entries(document.documentElement.dataset)) if (value != null) attrs[key] = value;
    return { css: document.getElementById('theme')?.textContent ?? '', attrs };
}

function post(frame: HTMLIFrameElement | null, message: ToFrame): void {
    // Окошко без своего происхождения: адресовать его можно только «*» — и только ему, по ссылке на его окно.
    frame?.contentWindow?.postMessage(message, '*');
}

/** Окошко, вставленное в пульт: создаётся скрытым для разведки или видимым для раздела и виджета. */
function makeFrame(hidden: boolean): HTMLIFrameElement {
    const frame = document.createElement('iframe');
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.setAttribute('referrerpolicy', 'no-referrer');
    frame.setAttribute('loading', 'eager');
    frame.src = FRAME_PAGE;
    frame.className = 'app-frame';
    if (hidden) {
        frame.setAttribute('aria-hidden', 'true');
        frame.tabIndex = -1;
        frame.style.cssText = 'position:absolute;width:1px;height:1px;border:0;opacity:0;pointer-events:none;left:-9999px;top:0';
    }
    return frame;
}

/** Что приложение приносит в пульт: его модуль загружается в скрытом окошке и описывает себя словами. */
export function probeShape(id: string, module: string, styles: string | null | undefined, state: FrameState): Promise<FrameShape> {
    return new Promise((resolve, reject) => {
        const frame = makeFrame(true);
        let done = false;
        const finish = (): void => {
            done = true;
            window.removeEventListener('message', onMessage);
            window.clearTimeout(timer);
            frame.remove();
        };
        const onMessage = (event: MessageEvent): void => {
            if (event.source !== frame.contentWindow || !ours(event.data)) return;
            const message = event.data as FromFrame;
            if (message.type === 'duowave:ready') post(frame, { type: 'duowave:init', module, styles, part: { kind: 'probe' }, state });
            else if (message.type === 'duowave:shape') {
                finish();
                resolve(message.shape);
            } else if (message.type === 'duowave:failed') {
                finish();
                reject(new Error(message.message));
            }
        };
        const timer = window.setTimeout(() => {
            if (done) return;
            finish();
            reject(new Error(tr.frame.probeSilent(id)));
        }, PROBE_MS);
        window.addEventListener('message', onMessage);
        document.body.appendChild(frame);
    });
}

/** Состояние пульта, каким его видит приложение `id`. */
function useFrameState(id: string, draft: boolean): FrameState {
    const version = usePluginVersion(id);
    const settings = useSettings(id);
    const station = useStation();
    const live = useLive();
    const theme = useTheme();
    const locale = useLanguage();
    return {
        locale,
        app: id,
        version,
        settings,
        permissions: ['plugins.manage', 'station.view', 'journal.view', 'settings.edit'].filter((permission) => can(permission)),
        station: { device: station.device, name: station.name, online: station.online, mode: station.mode, role: station.role },
        live: { connected: live.connected, presence: live.presence as Record<string, unknown> },
        theme: { ...themeNow(), attrs: { ...themeNow().attrs, theme: theme.mode } },
        draft,
    };
}

/**
 * Окошко приложения в пульте: его раздел или вклад в слот, нарисованный его кодом в изолированном фрейме. `call` — как
 * звать его функции (у черновика — песочница мастерской).
 */
export function AppFrame({
    id,
    module,
    styles,
    part,
    draft = false,
    call,
    minHeight = 48,
}: {
    id: string;
    module: string;
    styles?: string | null;
    part: Exclude<FramePart, { kind: 'probe' }>;
    draft?: boolean;
    call?: (fn: string, input: unknown) => Promise<unknown>;
    minHeight?: number;
}) {
    const box = useRef<HTMLDivElement>(null);
    const frameRef = useRef<HTMLIFrameElement | null>(null);
    const [height, setHeight] = useState(minHeight);
    const [failed, setFailed] = useState<string | null>(null);
    const panel = usePanel();
    const own = useApi(id);
    const state = useFrameState(id, draft);
    const stateRef = useRef(state);
    stateRef.current = state;
    const callRef = useRef(call ?? ((fn: string, input: unknown) => own.call(fn, input)));
    callRef.current = call ?? ((fn: string, input: unknown) => own.call(fn, input));

    useEffect(() => {
        const frame = makeFrame(false);
        frame.title = tr.frame.frameTitle(id);
        frame.style.cssText = `width:100%;border:0;display:block;height:${minHeight}px;background:transparent;color-scheme:inherit`;
        frameRef.current = frame;
        box.current?.appendChild(frame);
        let started = false;
        const midi = channelOf(id, draft);
        let midiHeld = false;
        let stopWatch: (() => void) | null = null;
        let stopEvents: (() => void) | null = null;
        // Долгие задачи приложения (tasks/state.ts): пульт читает их, только пока окошко следит (`useTask`).
        let stopTasks: (() => void) | null = null;
        const tellTasks = (): void => post(frame, { type: 'duowave:tasks-state', tasks: tasksOf(id).tasks.map(({ app: _app, ...task }) => task) });
        const midiState = () => ({
            state: midi.state,
            via: midi.via,
            why: midi.why,
            inputs: midi.inputs,
            outputs: midi.outputs,
            input: midi.input,
            output: midi.output,
            delayMs: midi.delayMs,
            browserPossible: 'requestMIDIAccess' in navigator,
        });
        const answer = (seq: number, work: Promise<unknown>): void => {
            work.then(
                (value) => post(frame, { type: 'duowave:answer', seq, ok: true, value }),
                (error) => post(frame, { type: 'duowave:answer', seq, ok: false, error: error instanceof Error ? error.message : String(error) }),
            );
        };
        const onMessage = (event: MessageEvent): void => {
            if (event.source !== frame.contentWindow || !ours(event.data)) return;
            const message = event.data as FromFrame;
            switch (message.type) {
                case 'duowave:ready':
                    // Окошко загрузилось заново (его код перешёл куда-то своим адресом) — второй раз ему ничего не дают.
                    if (started) {
                        setFailed(tr.frame.reloadedItself);
                        frame.remove();
                        return;
                    }
                    started = true;
                    post(frame, { type: 'duowave:init', module, styles, part, state: stateRef.current });
                    break;
                case 'duowave:size':
                    setHeight(Math.max(minHeight, Math.min(MAX_HEIGHT, Math.ceil(message.height))));
                    break;
                case 'duowave:failed':
                    setFailed(message.message);
                    break;
                case 'duowave:call':
                    answer(message.seq, callRef.current(String(message.fn), message.input ?? {}));
                    break;
                case 'duowave:file':
                    // A picture of the home folder (`useFolderFile`): made small, as the app for whoever looks.
                    answer(
                        message.seq,
                        message.address
                            ? fetchBusPreview(id, String(message.address), Math.min(Math.max(Number(message.size) || 960, 64), 2048))
                            : fetchPluginFile(id, String(message.name)),
                    );
                    break;
                case 'duowave:midi-file':
                    answer(message.seq, message.file instanceof Blob ? api.midiRead(message.file) : Promise.reject(new Error(tr.frame.notAFile)));
                    break;
                case 'duowave:ask':
                    answer(
                        message.seq,
                        ask({
                            title: tr.frame.asks(id, String(message.options?.title ?? '').slice(0, 120)),
                            text: message.options?.text ? String(message.options.text).slice(0, 600) : undefined,
                            ok: message.options?.ok ? String(message.options.ok).slice(0, 40) : undefined,
                            danger: !!message.options?.danger,
                        }),
                    );
                    break;
                case 'duowave:go':
                    panel.go(String(message.section), message.tab ? String(message.tab) : undefined, message.item ? String(message.item) : undefined);
                    break;
                case 'duowave:tasks':
                    // У черновика на примере задач нет: долгую работу ведёт только поставленное приложение.
                    if (message.op === 'retain' && !stopTasks) {
                        if (draft) post(frame, { type: 'duowave:tasks-state', tasks: [] });
                        else stopTasks = watchTasks(id, tellTasks);
                    } else if (message.op === 'release' && stopTasks) {
                        stopTasks();
                        stopTasks = null;
                    }
                    break;
                case 'duowave:task-cancel':
                    answer(message.seq, cancelTask(id, String(message.id)));
                    break;
                case 'duowave:midi': {
                    const args = Array.isArray(message.args) ? message.args : [];
                    if (message.op === 'retain' && !midiHeld) {
                        midiHeld = true;
                        midi.retain();
                        stopWatch = midi.watch(() => post(frame, { type: 'duowave:midi-state', state: midiState() }));
                        stopEvents = midi.on((midiEvent) => post(frame, { type: 'duowave:midi-event', event: midiEvent }));
                        post(frame, { type: 'duowave:midi-state', state: midiState() });
                    } else if (message.op === 'release' && midiHeld) {
                        midiHeld = false;
                        stopWatch?.();
                        stopEvents?.();
                        midi.releaseRef();
                    } else if (midiHeld) {
                        if (message.op === 'choose') midi.choose(String(args[0]));
                        else if (message.op === 'press') midi.press(Number(args[0]), args[1] == null ? undefined : Number(args[1]));
                        else if (message.op === 'release-note') midi.release(Number(args[0]));
                        else if (message.op === 'send' && args[0] && typeof args[0] === 'object') midi.send(args[0] as Parameters<typeof midi.send>[0]);
                        else if (message.op === 'play' && Array.isArray(args[0])) midi.play(args[0] as Array<[number, number, number, number]>, args[1] ? String(args[1]) : undefined);
                        else if (message.op === 'stop') midi.stop();
                        else if (message.op === 'browser') void midi.useBrowser();
                    }
                    break;
                }
            }
        };
        window.addEventListener('message', onMessage);
        return () => {
            window.removeEventListener('message', onMessage);
            stopTasks?.();
            if (midiHeld) {
                stopWatch?.();
                stopEvents?.();
                midi.releaseRef();
            }
            frame.remove();
            frameRef.current = null;
        };
    }, [id, module, styles, JSON.stringify(part), draft]);

    // Перемены пульта — окошку: тема, настройки, номер перемены его пакета.
    const stateKey = JSON.stringify(state);
    useEffect(() => {
        post(frameRef.current, { type: 'duowave:state', state });
    }, [stateKey]);

    useEffect(() => {
        if (frameRef.current) frameRef.current.style.height = `${height}px`;
    }, [height]);

    return (
        <div class="app-frame-box">
            <div ref={box} />
            {failed && <p class="t-caption m-0 c-dot-offline">{tr.frame.didNotOpen} {failed}</p>}
        </div>
    );
}

/** Значок по имени, которым его назвало окошко. */
function iconOf(name?: string) {
    return (name && (icons as Record<string, unknown>)[name]) || IconPackage;
}

/**
 * Плагин пульта из того, что окошко рассказало о себе: разделы и вклады — окошки его же кода. `id` — пакет, `key` —
 * имя плагина в реестре.
 */
export function framedPlugin(key: string, id: string, module: string, styles: string | null | undefined, shape: FrameShape): PanelPlugin {
    const sections: SectionDef[] = shape.sections.map((section) => {
        const Screen: FunctionComponent = () => h(AppFrame, { id, module, styles, part: { kind: 'section', id: section.id }, minHeight: 240 });
        return {
            id: section.id,
            title: section.title,
            icon: iconOf(section.icon) as SectionDef['icon'],
            order: section.order ?? 500,
            nav: section.nav,
            permission: section.permission,
            Screen,
        };
    });
    const slots: Record<string, SlotContribution[]> = {};
    for (const [name, list] of Object.entries(shape.slots)) {
        slots[name] = list.map((entry) => {
            const Component: FunctionComponent = () => h(AppFrame, { id, module, styles, part: { kind: 'slot', name, id: entry.id } });
            return {
                id: entry.id,
                title: entry.title,
                icon: entry.icon ? (iconOf(entry.icon) as SlotContribution['icon']) : undefined,
                order: entry.order,
                permission: entry.permission,
                size: entry.size,
                source: entry.source,
                initial: entry.initial,
                roles: entry.roles,
                bare: entry.bare,
                Component: Component as SlotContribution['Component'],
            };
        });
    }
    return { id: key, sections, slots };
}

/** Состояние для разведки: без пульта вокруг, одни настройки и тема. */
export function probeState(id: string, settings: Record<string, unknown>): FrameState {
    return {
        locale: language(),
        app: id,
        version: 0,
        settings,
        permissions: [],
        station: { device: null, name: '', online: false, mode: null, role: 'gateway' },
        live: { connected: false, presence: {} },
        theme: themeNow(),
        draft: false,
    };
}
