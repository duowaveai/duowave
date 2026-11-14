// Страница окошка приложения (frame.html; docs/plans/bus.md, «Окошки»). Ждёт от пульта, что показать, загружает модуль
// приложения и рисует его раздел или вклад; размер говорит пульту. Сама в сеть не ходит и не может: её правило —
// `sandbox allow-scripts`, `connect-src 'none'` (gateway/src/panel/controller.rs). Слушает только своего родителя.

import { Component, ComponentChildren, FunctionComponent, render } from 'preact';

import '@fontsource/inter-tight/300.css';
import '@fontsource/jetbrains-mono/400.css';
import '../ui/fonts-minimal.css';
import '../ui/glass.css';
import '../ui/packages.css';
import '../ui/duowave.css';
import '../ui/utilities.css';
import '../ui/minimal.css';
import '../ui/desk.css';
import '../ui/piano-keys.css';
import './frame.css';
import type { PanelPlugin } from '../shell/registry';
import type { SectionDef } from '../shell/sections';
import type { SlotContribution } from '../shell/slot';
import { FRAME_SLOTS, FrameShape, ToFrame, ours } from './protocol';
import { answered, iconName, installFrameSdk, midiEvent, send, setMidiState, setState, setTasks } from './sdk';
import { setLanguage, tr, useLanguage } from '../i18n';

// Ни одной лишней двери: соединения закрыты правилом страницы, а WebRTC правило не закрывает — убираем его до кода
// приложения.
for (const name of ['RTCPeerConnection', 'webkitRTCPeerConnection', 'RTCDataChannel', 'RTCSessionDescription', 'RTCIceCandidate']) {
    try {
        Object.defineProperty(globalThis, name, { value: undefined, configurable: false, writable: false });
    } catch {
        // нет такого — и не надо
    }
}

installFrameSdk();

class Boundary extends Component<{ children: ComponentChildren }, { failed: string | null }> {
    state = { failed: null as string | null };

    static getDerivedStateFromError(error: unknown): { failed: string } {
        return { failed: error instanceof Error ? error.message : String(error) };
    }

    componentDidCatch(error: unknown): void {
        console.error(tr.frame.appCrashedLog, error);
    }

    render() {
        return this.state.failed ? <p class="t-caption m-0 c-dot-offline">{tr.frame.appCrashed} {this.state.failed}</p> : this.props.children;
    }
}

/** Модуль только со своего шлюза: части приложений и черновиков, напрямую или через проход помощника. */
function allowed(module: string): string | null {
    try {
        const url = new URL(module, location.href);
        if (url.origin !== location.origin) return null;
        return /^(\/station\/[^/]+)?\/(panel-plugins|panel-preview)\//.test(url.pathname) ? url.href : null;
    } catch {
        return null;
    }
}

function shapeOf(plugin: PanelPlugin): FrameShape {
    const sections = (plugin.sections ?? []).map((section: SectionDef) => ({
        id: section.id,
        title: section.title,
        icon: iconName(section.icon),
        order: section.order,
        nav: section.nav,
        permission: section.permission,
    }));
    const slots: FrameShape['slots'] = {};
    for (const [name, list] of Object.entries(plugin.slots ?? {})) {
        if (!FRAME_SLOTS.includes(name) || !Array.isArray(list)) continue;
        slots[name] = list.map((entry: SlotContribution) => ({
            id: entry.id,
            title: entry.title,
            icon: entry.icon ? iconName(entry.icon) : undefined,
            order: entry.order,
            permission: entry.permission,
            size: entry.size,
            source: entry.source,
            initial: entry.initial,
            roles: entry.roles,
            bare: entry.bare,
        }));
    }
    return { sections, slots };
}

let started = false;

/** Высота содержимого — пульту, чтобы окошко было по нему. */
function reportSize(): void {
    let last = 0;
    const tell = (): void => {
        const height = Math.ceil(document.documentElement.scrollHeight);
        if (height !== last) {
            last = height;
            send({ type: 'duowave:size', height });
        }
    };
    new ResizeObserver(tell).observe(document.body);
    tell();
}

async function start(message: Extract<ToFrame, { type: 'duowave:init' }>): Promise<void> {
    if (started) return;
    started = true;
    setState(message.state);
    applyTheme(message.state.theme);
    // Язык пульта — до кода приложения: его тексты и детали SDK говорят на нём (docs/plans/i18n.md).
    await setLanguage(message.state.locale ?? 'ru', false);
    const module = allowed(message.module);
    if (!module) {
        send({ type: 'duowave:failed', message: tr.frame.moduleNotHere });
        return;
    }
    if (message.styles && allowed(message.styles)) {
        const link = document.createElement('link');
        link.rel = 'stylesheet';
        link.href = allowed(message.styles)!;
        document.head.appendChild(link);
    }
    let plugin: PanelPlugin;
    try {
        const loaded = await import(/* @vite-ignore */ module);
        plugin = loaded.default ?? loaded.plugin;
        if (!plugin || typeof plugin !== 'object') throw new Error(tr.frame.noPlugin);
    } catch (error) {
        send({ type: 'duowave:failed', message: error instanceof Error ? error.message : String(error) });
        return;
    }
    const part = message.part;
    if (part.kind === 'probe') {
        send({ type: 'duowave:shape', shape: shapeOf(plugin) });
        return;
    }
    let Body: FunctionComponent | undefined;
    if (part.kind === 'section') Body = plugin.sections?.find((section) => section.id === part.id)?.Screen;
    else Body = plugin.slots?.[part.name]?.find((entry) => entry.id === part.id)?.Component as FunctionComponent | undefined;
    if (!Body) {
        send({ type: 'duowave:failed', message: tr.frame.noPart(part.kind === 'section' ? part.id : `${part.name}/${part.id}`) });
        return;
    }
    const root = document.getElementById('root')!;
    const Shown = Body;
    // Смена языка пульта пересоздаёт окошко изнутри: и тексты приложения, и детали SDK — на новом языке.
    const Root = () => {
        const language = useLanguage();
        return (
            <Boundary key={language}>
                <Shown />
            </Boundary>
        );
    };
    render(<Root />, root);
    reportSize();
}

function applyTheme(theme: { css: string; attrs: Record<string, string> }): void {
    let style = document.getElementById('theme') as HTMLStyleElement | null;
    if (!style) {
        style = document.createElement('style');
        style.id = 'theme';
        document.head.appendChild(style);
    }
    if (style.textContent !== theme.css) style.textContent = theme.css;
    for (const [key, value] of Object.entries(theme.attrs)) document.documentElement.dataset[key] = value;
}

window.addEventListener('message', (event: MessageEvent) => {
    // Только родитель — пульт этого же шлюза.
    if (event.source !== window.parent || event.origin !== location.origin || !ours(event.data)) return;
    const message = event.data as ToFrame;
    switch (message.type) {
        case 'duowave:init':
            void start(message);
            break;
        case 'duowave:state':
            setState(message.state);
            applyTheme(message.state.theme);
            if (message.state.locale) void setLanguage(message.state.locale, false);
            break;
        case 'duowave:answer':
            answered(message.seq, message.ok, message.value, message.error);
            break;
        case 'duowave:midi-state':
            setMidiState(message.state);
            break;
        case 'duowave:midi-event':
            midiEvent(message.event);
            break;
        case 'duowave:tasks-state':
            setTasks(message.tasks);
            break;
    }
});

send({ type: 'duowave:ready' });
