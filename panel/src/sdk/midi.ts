// MIDI-клавиатура для приложений (docs/plans/midi.md): `useMidi()` в SDK пульта. Три пути, одни и те же события:
//
// - шлюз: клавиатура у компьютера с DuoWave, ноты — по своему каналу (`/api/v1/midi/ws`, билет на приложение). Работает
//   в приложении DuoWave (там у окна Web MIDI нет), в любом браузере и с планшета дома;
// - браузер: Web MIDI в Chromium, по кнопке («Подключить через браузер»), когда у шлюза клавиатуры нет — только на
//   `localhost` или https;
// - пример: в предпросмотре черновика («Примерка», мастерская) — клавиатура на экране, без устройства: до «да» своя
//   клавиатура не подключается.
//
// Канал один на приложение в странице, сколько бы его частей ни слушали; ушла последняя — закрыт. Страница спрятана
// дольше минуты — тоже. Ноты идут обработчику сразу, не через состояние Preact: клавиша не ждёт перерисовки.

import * as hooks from 'preact/hooks';

import { api } from '../api';
import { available, whyNot } from '../availability';
import { PluginScope } from '../plugins/versions';
import { DraftPreview } from './index';

export type MidiState = 'connecting' | 'ready' | 'nodevice' | 'browser' | 'off' | 'example';
export type MidiVia = 'gateway' | 'browser' | 'example';

/** Что сыграли: нота, отпускание, педаль (64), ручка, колесо, смена звука, нажим. `from` — клавиатура или экран. */
export interface MidiEvent {
    type: 'on' | 'off' | 'pedal' | 'cc' | 'bend' | 'program' | 'pressure' | 'touch';
    note?: number;
    velocity?: number;
    channel: number;
    control?: number;
    value?: number;
    /** Педаль нажата. */
    down?: boolean;
    /** Время шлюза или браузера, мс. */
    at: number;
    from: 'keyboard' | 'screen';
}

export interface MidiPort {
    id: string;
    name: string;
}

export interface MidiHandle {
    state: MidiState;
    via: MidiVia;
    /** Словами, что сделать, когда не «ready». */
    why: string | null;
    inputs: MidiPort[];
    outputs: MidiPort[];
    input: MidiPort | null;
    output: MidiPort | null;
    choose(id: string): void;
    /** Слушать события; отдаёт «перестать». */
    on(handler: (event: MidiEvent) => void): () => void;
    /** Клавиша на экране — как с клавиатуры. */
    press(note: number, velocity?: number): void;
    release(note: number): void;
    /** Сообщение в клавиатуру: `{ type: 'on', note, velocity }`, `{ type: 'cc', control, value }`… */
    send(event: Partial<MidiEvent> & { type: MidiEvent['type'] }): void;
    /** Мелодия на клавиатуру: `[нота, сила, начало мс, длина мс]`, играет шлюз или браузер по своим часам. */
    play(notes: Array<[number, number, number, number]>, options?: { id?: string }): void;
    stop(): void;
    /** «Подключить через браузер»: Chrome спросит разрешение. Только по нажатию. */
    useBrowser(): Promise<void>;
    /** Можно ли подключить клавиатуру через браузер. */
    browserPossible: boolean;
    /** До шлюза, мс, по пингу; null — не знаем. */
    delayMs: number | null;
}

type Listener = (event: MidiEvent) => void;

const CHOSEN_KEY = 'duowave.midi.input.';
const PING_MS = 5000;
const HIDDEN_MS = 60_000;

/** Web MIDI возможен: Chromium и «безопасная» страница. */
function browserPossible(): boolean {
    return typeof navigator !== 'undefined' && 'requestMIDIAccess' in navigator && window.isSecureContext;
}

/** Почему через браузер нельзя — словами. */
function browserWhy(): string {
    if (!('requestMIDIAccess' in navigator))
        return 'В этом окне MIDI в браузере нет (приложение DuoWave, Safari): клавиатуру слышит шлюз на компьютере.';
    return 'На странице по адресу дома (http) браузер MIDI не даёт: откройте пульт на компьютере с клавиатурой.';
}

/** Сообщение MIDI байтами — событие; не канальное (SysEx, такт) — ничего. */
export function eventOf(bytes: ArrayLike<number>, at: number): MidiEvent | null {
    const status = bytes[0];
    if (status === undefined || status < 0x80 || status >= 0xf0) return null;
    const channel = (status & 0x0f) + 1;
    const a = bytes[1] ?? 0;
    const b = bytes[2] ?? 0;
    const base = { channel, at, from: 'keyboard' as const };
    switch (status & 0xf0) {
        case 0x80:
            return { ...base, type: 'off', note: a, velocity: b };
        case 0x90:
            return b === 0 ? { ...base, type: 'off', note: a, velocity: 0 } : { ...base, type: 'on', note: a, velocity: b };
        case 0xa0:
            return { ...base, type: 'touch', note: a, value: b };
        case 0xb0:
            return a === 64 ? { ...base, type: 'pedal', control: 64, value: b, down: b >= 64 } : { ...base, type: 'cc', control: a, value: b };
        case 0xc0:
            return { ...base, type: 'program', value: a };
        case 0xd0:
            return { ...base, type: 'pressure', value: a };
        default:
            return { ...base, type: 'bend', value: (a | (b << 7)) - 8192 };
    }
}

/** Событие — байты для клавиатуры. */
function bytesOf(event: Partial<MidiEvent> & { type: MidiEvent['type'] }): number[] {
    const channel = Math.min(16, Math.max(1, event.channel ?? 1)) - 1;
    const note = event.note ?? 60;
    switch (event.type) {
        case 'on':
            return [0x90 | channel, note, event.velocity ?? 100];
        case 'off':
            return [0x80 | channel, note, 0];
        case 'touch':
            return [0xa0 | channel, note, event.value ?? 0];
        case 'pedal':
            return [0xb0 | channel, 64, event.down ? 127 : 0];
        case 'cc':
            return [0xb0 | channel, event.control ?? 0, event.value ?? 0];
        case 'program':
            return [0xc0 | channel, event.value ?? 0];
        case 'pressure':
            return [0xd0 | channel, event.value ?? 0];
        default: {
            const raw = Math.min(8191, Math.max(-8192, event.value ?? 0)) + 8192;
            return [0xe0 | channel, raw & 0x7f, raw >> 7];
        }
    }
}

/** Кадр шлюза — событие. */
function eventOfFrame(frame: Record<string, unknown>): MidiEvent | null {
    const t = frame.t as string;
    const channel = Number(frame.c ?? 1);
    const at = Number(frame.at ?? 0);
    const base = { channel, at, from: 'keyboard' as const };
    switch (t) {
        case 'on':
        case 'off':
            return { ...base, type: t, note: Number(frame.n), velocity: Number(frame.v ?? 0) };
        case 'touch':
            return { ...base, type: 'touch', note: Number(frame.n), value: Number(frame.v) };
        case 'cc':
            return Number(frame.k) === 64
                ? { ...base, type: 'pedal', control: 64, value: Number(frame.v), down: Number(frame.v) >= 64 }
                : { ...base, type: 'cc', control: Number(frame.k), value: Number(frame.v) };
        case 'program':
            return { ...base, type: 'program', value: Number(frame.p) };
        case 'pressure':
        case 'bend':
            return { ...base, type: t, value: Number(frame.v) };
        default:
            return null;
    }
}

/** Событие — кадр в шлюз. */
function frameOf(event: Partial<MidiEvent> & { type: MidiEvent['type'] }, device: string): Record<string, unknown> {
    const c = event.channel ?? 1;
    switch (event.type) {
        case 'on':
            return { t: 'on', d: device, c, n: event.note, v: event.velocity ?? 100 };
        case 'off':
            return { t: 'off', d: device, c, n: event.note, v: 0 };
        case 'touch':
            return { t: 'touch', d: device, c, n: event.note, v: event.value ?? 0 };
        case 'pedal':
            return { t: 'cc', d: device, c, k: 64, v: event.down ? 127 : 0 };
        case 'cc':
            return { t: 'cc', d: device, c, k: event.control ?? 0, v: event.value ?? 0 };
        case 'program':
            return { t: 'program', d: device, c, p: event.value ?? 0 };
        default:
            return { t: event.type, d: device, c, v: event.value ?? 0 };
    }
}

interface Device {
    name: string;
    input?: string;
    output?: string;
}

/** Канал одного приложения в этой странице. */
class Channel {
    state: MidiState = 'connecting';
    via: MidiVia = 'gateway';
    why: string | null = null;
    inputs: MidiPort[] = [];
    outputs: MidiPort[] = [];
    input: MidiPort | null = null;
    output: MidiPort | null = null;
    delayMs: number | null = null;
    version = 0;

    private listeners = new Set<Listener>();
    private watchers = new Set<() => void>();
    private refs = 0;
    private ws: WebSocket | null = null;
    private token: string | null = null;
    private lastSeq = 0;
    private ping: ReturnType<typeof setInterval> | undefined;
    private retry: ReturnType<typeof setTimeout> | undefined;
    private retries = 0;
    private hiddenTimer: ReturnType<typeof setTimeout> | undefined;
    private closedByHiding = false;
    private access: MIDIAccess | null = null;
    private browserHeld = new Set<number>();
    private browserTimers: ReturnType<typeof setTimeout>[] = [];

    constructor(
        readonly plugin: string,
        readonly example: boolean,
    ) {
        if (example) {
            this.state = 'example';
            this.via = 'example';
            this.why = 'Пример: клавиатура на экране. Своя клавиатура подключится после «да».';
        }
    }

    private changed(): void {
        this.version += 1;
        this.watchers.forEach((watch) => watch());
    }

    watch(watcher: () => void): () => void {
        this.watchers.add(watcher);
        return () => this.watchers.delete(watcher);
    }

    on(listener: Listener): () => void {
        this.listeners.add(listener);
        return () => this.listeners.delete(listener);
    }

    emit(event: MidiEvent): void {
        this.listeners.forEach((listener) => {
            try {
                listener(event);
            } catch (error) {
                console.error(`MIDI (${this.plugin}):`, error);
            }
        });
    }

    retain(): void {
        this.refs += 1;
        if (this.refs === 1) {
            document.addEventListener('visibilitychange', this.onVisibility);
            if (!this.example) this.start();
        }
    }

    releaseRef(): void {
        this.refs -= 1;
        if (this.refs > 0) return;
        document.removeEventListener('visibilitychange', this.onVisibility);
        this.shut();
    }

    private onVisibility = (): void => {
        if (document.hidden) {
            clearTimeout(this.hiddenTimer);
            this.hiddenTimer = setTimeout(() => {
                if (!document.hidden || this.refs === 0) return;
                this.closedByHiding = true;
                this.shut();
            }, HIDDEN_MS);
        } else {
            clearTimeout(this.hiddenTimer);
            if (this.closedByHiding && this.refs > 0) {
                this.closedByHiding = false;
                this.start();
            }
        }
    };

    private start(): void {
        if (this.via === 'browser' && this.access) return;
        if (!available('plugins.midi')) {
            this.via = 'gateway';
            this.state = browserPossible() ? 'browser' : 'off';
            this.why = browserPossible()
                ? 'У шлюза клавиатуры нет. Клавиатура у этого устройства? Подключите её через браузер.'
                : `${whyNot('plugins.midi') ?? 'У шлюза клавиатуры нет.'} ${browserWhy()}`;
            this.changed();
            return;
        }
        void this.connect(null);
    }

    /** Новый сокет; `resume` — токен прежнего: шлюз перенесёт на новый всё, что было у прежнего. */
    private async connect(resume: string | null): Promise<void> {
        let ticket: string;
        try {
            ticket = (await api.midiTicket(this.plugin)).ticket;
        } catch (error) {
            this.state = 'off';
            this.why = error instanceof Error ? error.message : String(error);
            this.changed();
            return;
        }
        if (this.refs === 0) return;
        const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/v1/midi/ws?ticket=${encodeURIComponent(ticket)}`);
        const previous = resume ? this.ws : null;
        ws.onmessage = (message) => this.onFrame(ws, JSON.parse(String(message.data)) as Record<string, unknown>, resume, previous);
        ws.onclose = () => {
            if (this.ws !== ws) return;
            this.ws = null;
            clearInterval(this.ping);
            if (this.refs === 0 || this.closedByHiding) return;
            // Сеть моргнула: шлюз ждёт страницу 15 с, тот же токен вернёт всё, что было.
            this.state = 'connecting';
            this.changed();
            const wait = [1000, 2000, 5000][Math.min(this.retries, 2)];
            this.retries += 1;
            clearTimeout(this.retry);
            this.retry = setTimeout(() => void this.connect(this.token), wait);
        };
        if (!resume) this.ws = ws;
    }

    private onFrame(ws: WebSocket, frame: Record<string, unknown>, resume: string | null, previous: WebSocket | null): void {
        const t = frame.t as string;
        if (t === 'hello') {
            this.retries = 0;
            const oldToken = resume;
            this.token = frame.token as string;
            if (oldToken) {
                ws.send(JSON.stringify({ t: 'resume', token: oldToken }));
            }
            this.ws = ws;
            this.devices(frame.devices as Device[], !oldToken);
            clearInterval(this.ping);
            this.ping = setInterval(() => ws.readyState === WebSocket.OPEN && ws.send(JSON.stringify({ t: 'ping', at: performance.now() })), PING_MS);
            ws.send(JSON.stringify({ t: 'ping', at: performance.now() }));
            return;
        }
        if (t === 'resumed') {
            if (previous && previous !== ws) previous.close(1000, 'renewed');
            return;
        }
        if (t === 'renew') {
            // Продление: новый канал с токеном этого, этот закроется, когда шлюз всё перенёс.
            if (this.token) void this.connect(this.token);
            return;
        }
        if (t === 'devices') {
            this.devices(frame.devices as Device[], true);
            return;
        }
        if (t === 'pong') {
            this.delayMs = Math.round((performance.now() - Number(frame.at)) / 2);
            this.changed();
            return;
        }
        if (t === 'why' && frame.of === 'resume') {
            // Прежний канал уже закрыт (страница вернулась позже 15 с): слушаем заново.
            if (this.input) ws.send(JSON.stringify({ t: 'listen', d: this.input.id }));
            return;
        }
        if (t === 'why') {
            // Прежний сокет после продления говорит «канал перешёл на новую страницу» — это не о нынешнем канале.
            if (ws !== this.ws) return;
            this.why = String(frame.text);
            if (frame.end) this.state = 'off';
            this.changed();
            return;
        }
        if (t === 'listening' || t === 'playing' || t === 'played' || t === 'lost') return;
        const seq = Number(frame.s ?? 0);
        if (seq && seq <= this.lastSeq) return;
        if (seq) this.lastSeq = seq;
        const event = eventOfFrame(frame);
        if (event && frame.d === this.input?.id) this.emit(event);
    }

    /** Список клавиатур шлюза; выбранная — по имени из памяти браузера, иначе первая со входом. */
    private devices(devices: Device[], listen: boolean): void {
        this.inputs = devices.filter((device) => device.input).map((device) => ({ id: device.input as string, name: device.name }));
        this.outputs = devices.filter((device) => device.output).map((device) => ({ id: device.output as string, name: device.name }));
        let remembered: string | null = null;
        try {
            remembered = localStorage.getItem(CHOSEN_KEY + this.plugin);
        } catch {
            remembered = null;
        }
        const was = this.input;
        this.input = this.inputs.find((port) => port.name === (was?.name ?? remembered)) ?? this.inputs[0] ?? null;
        this.output = this.outputs.find((port) => port.name === this.input?.name) ?? this.outputs[0] ?? null;
        if (this.input) {
            this.state = 'ready';
            this.why = null;
            if (listen || was?.id !== this.input.id) this.ws?.send(JSON.stringify({ t: 'listen', d: this.input.id }));
        } else {
            this.state = 'nodevice';
            this.why = browserPossible()
                ? 'Подключите клавиатуру к компьютеру с DuoWave (USB). Клавиатура у этого устройства? Подключите через браузер.'
                : 'Подключите клавиатуру к компьютеру с DuoWave (USB).';
        }
        this.changed();
    }

    choose(id: string): void {
        const port = this.inputs.find((input) => input.id === id);
        if (!port) return;
        try {
            localStorage.setItem(CHOSEN_KEY + this.plugin, port.name);
        } catch {
            // Память браузера закрыта — выбор живёт до перезагрузки.
        }
        if (this.via === 'browser') {
            this.input = port;
            this.output = this.outputs.find((output) => output.name === port.name) ?? this.output;
            this.bindBrowser();
            this.changed();
            return;
        }
        if (this.input) this.ws?.send(JSON.stringify({ t: 'unlisten', d: this.input.id }));
        this.input = port;
        this.output = this.outputs.find((output) => output.name === port.name) ?? this.output;
        this.ws?.send(JSON.stringify({ t: 'listen', d: port.id }));
        this.changed();
    }

    press(note: number, velocity = 96): void {
        this.emit({ type: 'on', note, velocity, channel: 1, at: performance.now(), from: 'screen' });
    }

    release(note: number): void {
        this.emit({ type: 'off', note, velocity: 0, channel: 1, at: performance.now(), from: 'screen' });
    }

    send(event: Partial<MidiEvent> & { type: MidiEvent['type'] }): void {
        if (!this.output) return;
        if (this.via === 'browser') {
            const output = this.access?.outputs.get(this.output.id);
            output?.send(bytesOf(event));
            if (event.type === 'on' && event.note != null) this.browserHeld.add(event.note);
            if (event.type === 'off' && event.note != null) this.browserHeld.delete(event.note);
            return;
        }
        this.ws?.send(JSON.stringify(frameOf(event, this.output.id)));
    }

    play(notes: Array<[number, number, number, number]>, id = 'play'): void {
        if (!this.output) return;
        if (this.via === 'browser') {
            this.stop();
            const start = performance.now() + 50;
            const output = this.access?.outputs.get(this.output.id);
            for (const [note, velocity, at, length] of notes) {
                output?.send([0x90, note, velocity], start + at);
                output?.send([0x80, note, 0], start + at + Math.max(1, length));
            }
            return;
        }
        this.ws?.send(JSON.stringify({ t: 'play', id, d: this.output.id, notes }));
    }

    stop(): void {
        if (this.via === 'browser') {
            const output = this.output ? this.access?.outputs.get(this.output.id) : undefined;
            // Ноты мелодии, ещё не ушедшие в клавиатуру, — снять (в Chrome есть, в типах — ещё нет).
            (output as (MIDIOutput & { clear?: () => void }) | undefined)?.clear?.();
            this.browserHeld.forEach((note) => output?.send([0x80, note, 0]));
            this.browserHeld.clear();
            output?.send([0xb0, 123, 0]);
            this.browserTimers.forEach(clearTimeout);
            this.browserTimers = [];
            return;
        }
        this.ws?.send(JSON.stringify({ t: 'stop' }));
    }

    /** Запасной путь: Web MIDI браузера. Ноты через шлюз не идут вовсе. */
    async useBrowser(): Promise<void> {
        if (!browserPossible()) {
            this.why = browserWhy();
            this.changed();
            return;
        }
        try {
            this.access = await navigator.requestMIDIAccess({ sysex: false });
        } catch {
            this.state = 'off';
            this.why = 'Браузер не разрешил MIDI: разрешение — в настройках сайта (значок замка в адресной строке).';
            this.changed();
            return;
        }
        this.shutSocket();
        this.via = 'browser';
        this.access.onstatechange = () => this.browserPorts();
        this.browserPorts();
    }

    private browserPorts(): void {
        if (!this.access) return;
        this.inputs = [...this.access.inputs.values()].map((port) => ({ id: port.id, name: port.name ?? port.id }));
        this.outputs = [...this.access.outputs.values()].map((port) => ({ id: port.id, name: port.name ?? port.id }));
        this.input = this.inputs.find((port) => port.id === this.input?.id) ?? this.inputs[0] ?? null;
        this.output = this.outputs.find((port) => port.name === this.input?.name) ?? this.outputs[0] ?? null;
        this.state = this.input ? 'ready' : 'nodevice';
        this.why = this.input ? null : 'Браузер не видит клавиатуру: подключите её к этому устройству.';
        this.bindBrowser();
        this.changed();
    }

    private bindBrowser(): void {
        if (!this.access) return;
        this.access.inputs.forEach((port) => {
            port.onmidimessage = port.id === this.input?.id ? (message) => {
                const event = message.data ? eventOf(message.data, message.timeStamp) : null;
                if (event) this.emit(event);
            } : null;
        });
    }

    private shutSocket(): void {
        clearInterval(this.ping);
        clearTimeout(this.retry);
        const ws = this.ws;
        this.ws = null;
        ws?.close(1000, 'bye');
    }

    private shut(): void {
        this.shutSocket();
        if (this.access) {
            this.stop();
            this.access.inputs.forEach((port) => {
                port.onmidimessage = null;
            });
            this.access.onstatechange = null;
            this.access = null;
            this.via = 'gateway';
        }
        if (!this.example) {
            this.state = 'connecting';
            this.changed();
        }
    }
}

const channels = new Map<string, Channel>();

function channelOf(plugin: string, example: boolean): Channel {
    const key = `${example ? 'example:' : ''}${plugin}`;
    let channel = channels.get(key);
    if (!channel) {
        channel = new Channel(plugin, example);
        channels.set(key, channel);
    }
    return channel;
}

/**
 * MIDI-клавиатура своего приложения (право `midi`, Host API 1.3). Одна на все части приложения в странице:
 *
 *   const midi = useMidi();
 *   useEffect(() => midi.on((e) => { if (e.type === 'on') synth.down(e.note, e.velocity); }), [midi]);
 */
export function useMidi(): MidiHandle {
    const plugin = hooks.useContext(PluginScope) ?? 'unknown';
    const draft = hooks.useContext(DraftPreview);
    const channel = channelOf(plugin, draft != null);
    const [, rerender] = hooks.useState(0);
    hooks.useEffect(() => {
        const stop = channel.watch(() => rerender((n) => n + 1));
        channel.retain();
        return () => {
            stop();
            channel.releaseRef();
        };
    }, [channel]);
    return hooks.useMemo<MidiHandle>(
        () => ({
            state: channel.state,
            via: channel.via,
            why: channel.why,
            inputs: channel.inputs,
            outputs: channel.outputs,
            input: channel.input,
            output: channel.output,
            delayMs: channel.delayMs,
            browserPossible: browserPossible(),
            choose: (id) => channel.choose(id),
            on: (handler) => channel.on(handler),
            press: (note, velocity) => channel.press(note, velocity),
            release: (note) => channel.release(note),
            send: (event) => channel.send(event),
            play: (notes, options) => channel.play(notes, options?.id),
            stop: () => channel.stop(),
            useBrowser: () => channel.useBrowser(),
        }),
        [channel, channel.version],
    );
}

/** Ноты по-русски: «до», «до♯», …, «си». */
const NAMES = ['до', 'до♯', 'ре', 'ре♯', 'ми', 'фа', 'фа♯', 'соль', 'соль♯', 'ля', 'ля♯', 'си'];
const OCTAVES = ['субконтроктавы', 'контроктавы', 'большой октавы', 'малой октавы', 'первой октавы', 'второй октавы', 'третьей октавы', 'четвёртой октавы', 'пятой октавы'];

/** `noteName(60)` → «до первой октавы»; `noteName(60, true)` → «до». */
export function noteName(note: number, short = false): string {
    const name = NAMES[((note % 12) + 12) % 12];
    if (short) return name;
    const octave = OCTAVES[Math.floor(note / 12) - 1];
    return octave ? `${name} ${octave}` : name;
}

export interface MidiTrack {
    index: number;
    name?: string;
    drums: boolean;
    notes: number;
    /** `[нота, начало, длина]` в долях. */
    melody: Array<[number, number, number]>;
}

export interface MidiSong {
    tempo: number;
    meter: [number, number];
    tracks: MidiTrack[];
}

/** Файл `.mid` — дорожки с мелодиями (читает шлюз, файл нигде не хранится). Ошибка — словами. */
export async function readMidiFile(file: Blob): Promise<MidiSong> {
    if (file.size > 256 * 1024) throw new Error('Файл больше 256 КБ — это не простая мелодия.');
    return api.midiRead(file);
}

/** Мелодия дорожки: верхний голос уже взят шлюзом; здесь — по желанию сдвиг, чтобы начиналась с нуля. */
export function melodyOf(track: MidiTrack): Array<[number, number, number]> {
    const first = track.melody[0]?.[1] ?? 0;
    return track.melody.map(([note, start, length]) => [note, Math.round((start - first) * 1000) / 1000, length]);
}
