// Звук пульта для приложений (docs/plans/midi.md, «Звук — useSynth()»): простой «рояль» на Web Audio. Две волны —
// треугольник и тихий синус октавой выше; атака 5 мс, затухание около 1,2 с, отпускание 0,25 с; сила — громкость; до 16
// голосов (лишний гасит самый старый); педаль держит звук; ограничитель на выходе, чтобы аккорд не хрипел.
//
// Браузеры (и окно приложения) включают звук только после клика или касания на странице, а нажатие на MIDI-клавиатуре
// таким не считается: `needsTap` — пора попросить «Начать». Мелодию `playAll` играет по часам AudioContext: ровно, даже
// когда страница в фоне.

import * as hooks from 'preact/hooks';

const VOICES = 16;
const ATTACK = 0.005;
const DECAY = 0.4;
const RELEASE = 0.08;

interface Voice {
    note: number;
    gain: GainNode;
    oscillators: OscillatorNode[];
    held: boolean;
}

let context: AudioContext | null = null;
let master: GainNode | null = null;
const voices: Voice[] = [];
let pedal = false;
const watchers = new Set<() => void>();
let scheduled: Array<{ oscillators: OscillatorNode[]; gain: GainNode }> = [];
let timers: ReturnType<typeof setTimeout>[] = [];
let listening = false;

function changed(): void {
    watchers.forEach((watch) => watch());
}

/** Контекст звука страницы: один на всех, создаётся при первой нужде. */
function audio(): AudioContext | null {
    if (context) return context;
    const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctor) return null;
    context = new Ctor({ latencyHint: 'interactive' });
    const limiter = context.createDynamicsCompressor();
    limiter.threshold.value = -10;
    limiter.ratio.value = 12;
    master = context.createGain();
    master.gain.value = 0.8;
    master.connect(limiter).connect(context.destination);
    context.onstatechange = changed;
    if (!listening) {
        listening = true;
        // Первое касание страницы включает звук.
        const resume = (): void => {
            if (context && context.state !== 'running') void context.resume().then(changed);
        };
        document.addEventListener('pointerdown', resume, true);
        document.addEventListener('keydown', resume, true);
    }
    return context;
}

const frequency = (note: number): number => 440 * 2 ** ((note - 69) / 12);

/** Голос ноты: начинается в `at` (время контекста). */
function voice(ctx: AudioContext, note: number, velocity: number, at: number): { gain: GainNode; oscillators: OscillatorNode[] } {
    const peak = 0.45 * Math.pow(Math.min(127, Math.max(1, velocity)) / 127, 1.5);
    const gain = ctx.createGain();
    gain.gain.setValueAtTime(0, at);
    gain.gain.linearRampToValueAtTime(peak, at + ATTACK);
    gain.gain.setTargetAtTime(peak * 0.2, at + ATTACK, DECAY);
    gain.connect(master as GainNode);
    const oscillators: OscillatorNode[] = [];
    for (const [type, multiple, level] of [
        ['triangle', 1, 1],
        ['sine', 2, 0.25],
    ] as const) {
        const oscillator = ctx.createOscillator();
        oscillator.type = type;
        oscillator.frequency.value = frequency(note) * multiple;
        const part = ctx.createGain();
        part.gain.value = level;
        oscillator.connect(part).connect(gain);
        oscillator.start(at);
        oscillators.push(oscillator);
    }
    return { gain, oscillators };
}

function fade(ctx: AudioContext, gain: GainNode, oscillators: OscillatorNode[], at: number): void {
    gain.gain.cancelScheduledValues(at);
    gain.gain.setValueAtTime(gain.gain.value, at);
    gain.gain.setTargetAtTime(0, at, RELEASE);
    oscillators.forEach((oscillator) => oscillator.stop(at + RELEASE * 6));
}

function down(note: number, velocity = 96): void {
    const ctx = audio();
    if (!ctx || !master) return;
    const now = ctx.currentTime;
    // Та же нота ещё звучит — её место занимает новая.
    for (let i = voices.length - 1; i >= 0; i -= 1) {
        if (voices[i].note === note) {
            fade(ctx, voices[i].gain, voices[i].oscillators, now);
            voices.splice(i, 1);
        }
    }
    while (voices.length >= VOICES) {
        const oldest = voices.shift() as Voice;
        fade(ctx, oldest.gain, oldest.oscillators, now);
    }
    voices.push({ note, held: false, ...voice(ctx, note, velocity, now) });
}

function up(note: number): void {
    const ctx = context;
    if (!ctx) return;
    for (let i = voices.length - 1; i >= 0; i -= 1) {
        if (voices[i].note !== note) continue;
        if (pedal) {
            voices[i].held = true;
            continue;
        }
        fade(ctx, voices[i].gain, voices[i].oscillators, ctx.currentTime);
        voices.splice(i, 1);
    }
}

function setPedal(on: boolean): void {
    pedal = on;
    if (on || !context) return;
    const now = context.currentTime;
    for (let i = voices.length - 1; i >= 0; i -= 1) {
        if (!voices[i].held) continue;
        fade(context, voices[i].gain, voices[i].oscillators, now);
        voices.splice(i, 1);
    }
}

function stop(): void {
    timers.forEach(clearTimeout);
    timers = [];
    const ctx = context;
    if (!ctx) return;
    const now = ctx.currentTime;
    scheduled.forEach(({ gain, oscillators }) => {
        try {
            fade(ctx, gain, oscillators, now);
        } catch {
            // Уже доиграл.
        }
    });
    scheduled = [];
    voices.splice(0).forEach((v) => fade(ctx, v.gain, v.oscillators, now));
}

/** Мелодия `[нота, начало, длина]` в долях при `tempo` долей в минуту; `onNote(i)` — когда звучит i-я. */
function playAll(
    notes: Array<[number, number, number]>,
    options: { tempo?: number; velocity?: number; onNote?: (i: number) => void; onEnd?: () => void } = {},
): void {
    stop();
    const ctx = audio();
    if (!ctx || !master) return;
    const beat = 60 / (options.tempo ?? 100);
    const start = ctx.currentTime + 0.1;
    const startedAt = performance.now() + 100;
    notes.forEach(([note, at, length], i) => {
        const begins = start + at * beat;
        const sound = voice(ctx, note, options.velocity ?? 90, begins);
        fade(ctx, sound.gain, sound.oscillators, begins + Math.max(0.05, length * beat * 0.95));
        scheduled.push(sound);
        if (options.onNote) timers.push(setTimeout(() => options.onNote?.(i), startedAt - performance.now() + at * beat * 1000));
    });
    const end = notes.reduce((last, [, at, length]) => Math.max(last, at + length), 0);
    timers.push(
        setTimeout(() => {
            scheduled = [];
            options.onEnd?.();
        }, startedAt - performance.now() + end * beat * 1000),
    );
}

export interface SynthHandle {
    /** Звук включится после нажатия на странице (так у браузеров). */
    needsTap: boolean;
    /** Звук в этом окне есть. */
    supported: boolean;
    start(): Promise<void>;
    down(note: number, velocity?: number): void;
    up(note: number): void;
    pedal(on: boolean): void;
    playAll(notes: Array<[number, number, number]>, options?: { tempo?: number; velocity?: number; onNote?: (i: number) => void; onEnd?: () => void }): void;
    stop(): void;
    /** Сколько звук идёт до динамика, мс, если браузер говорит; больше 80 — наушники по Bluetooth и т. п. */
    delayMs: number | null;
}

/** Звук пульта: `synth.down(60, 96)`, `synth.up(60)`, `synth.playAll(мелодия, { tempo, onNote })`. */
export function useSynth(): SynthHandle {
    const [, rerender] = hooks.useState(0);
    hooks.useEffect(() => {
        const watch = (): void => rerender((n) => n + 1);
        watchers.add(watch);
        return () => {
            watchers.delete(watch);
        };
    }, []);
    const ctx = typeof window !== 'undefined' ? audio() : null;
    const latency = ctx ? ctx.baseLatency + ((ctx as AudioContext & { outputLatency?: number }).outputLatency ?? 0) : NaN;
    return {
        needsTap: !ctx || ctx.state !== 'running',
        supported: ctx != null,
        start: async () => {
            const c = audio();
            if (c && c.state !== 'running') await c.resume();
            changed();
        },
        down,
        up,
        pedal: setPedal,
        playAll,
        stop,
        delayMs: Number.isFinite(latency) && latency > 0 ? Math.round(latency * 1000) : null,
    };
}
