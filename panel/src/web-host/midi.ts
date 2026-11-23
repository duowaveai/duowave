// MIDI-клавиатура приложения на витрине сайта: шлюза нет, поэтому путь один — Web MIDI браузера (Chromium, https или
// localhost), и только по нажатию «Подключить через браузер». Клавиши на экране работают всегда. Состояние и события —
// те же, что у канала пульта (sdk/midi.ts): окошко приложения не знает, кто его хозяин.

import { bytesOf, eventOf } from '../sdk/midi-bytes';
import type { MidiEvent, MidiPort } from '../sdk/midi';
import type { FrameMidiState } from '../frame/protocol';

/** Слова канала: на языке страницы. */
export interface MidiWords {
    connect: string;
    noBrowserMidi: string;
    notSecure: string;
    denied: string;
    noKeyboard: string;
}

export const MIDI_WORDS: Record<'en' | 'ru', MidiWords> = {
    en: {
        connect: 'Play the keys on screen, or connect a MIDI keyboard through the browser.',
        noBrowserMidi: 'This browser has no Web MIDI: play the keys on screen, or open the page in Chrome or Edge.',
        notSecure: 'Web MIDI works only on https: play the keys on screen.',
        denied: 'The browser did not allow MIDI: play the keys on screen.',
        noKeyboard: 'The browser sees no MIDI keyboard: plug one in, or play the keys on screen.',
    },
    ru: {
        connect: 'Играйте клавишами на экране или подключите MIDI-клавиатуру через браузер.',
        noBrowserMidi: 'В этом браузере нет Web MIDI: играйте клавишами на экране или откройте страницу в Chrome или Edge.',
        notSecure: 'Web MIDI работает только по https: играйте клавишами на экране.',
        denied: 'Браузер не разрешил MIDI: играйте клавишами на экране.',
        noKeyboard: 'Браузер не видит MIDI-клавиатуру: подключите её или играйте клавишами на экране.',
    },
};

export function browserPossible(): boolean {
    return typeof navigator !== 'undefined' && 'requestMIDIAccess' in navigator && globalThis.isSecureContext === true;
}

/** Канал одного окошка: состояние — окошку, события — его обработчикам. */
export class WebMidi {
    private access: MIDIAccess | null = null;
    private input: MidiPort | null = null;
    private output: MidiPort | null = null;
    private inputs: MidiPort[] = [];
    private outputs: MidiPort[] = [];
    private state: string;
    private why: string | null;
    private held = new Set<number>();

    constructor(
        private readonly words: MidiWords,
        private readonly tell: (state: FrameMidiState) => void,
        private readonly emit: (event: MidiEvent) => void,
    ) {
        const possible = browserPossible();
        this.state = possible ? 'browser' : 'off';
        this.why = possible ? words.connect : 'requestMIDIAccess' in (globalThis.navigator ?? {}) ? words.notSecure : words.noBrowserMidi;
    }

    snapshot(): FrameMidiState {
        return {
            state: this.state,
            via: this.access ? 'browser' : 'example',
            why: this.why,
            inputs: this.inputs,
            outputs: this.outputs,
            input: this.input,
            output: this.output,
            delayMs: null,
            browserPossible: browserPossible(),
        };
    }

    private changed(): void {
        this.tell(this.snapshot());
    }

    /** Сообщение окошка (`duowave:midi`): то же, что мост пульта (frame/host.tsx). */
    handle(op: string, args: unknown[]): void {
        if (op === 'retain') this.changed();
        else if (op === 'release') this.close();
        else if (op === 'press') this.emit({ type: 'on', note: Number(args[0]), velocity: args[1] == null ? 96 : Number(args[1]), channel: 1, at: performance.now(), from: 'screen' });
        else if (op === 'release-note') this.emit({ type: 'off', note: Number(args[0]), velocity: 0, channel: 1, at: performance.now(), from: 'screen' });
        else if (op === 'choose') this.choose(String(args[0]));
        else if (op === 'send' && args[0] && typeof args[0] === 'object') this.send(args[0] as Partial<MidiEvent> & { type: MidiEvent['type'] });
        else if (op === 'play' && Array.isArray(args[0])) this.play(args[0] as Array<[number, number, number, number]>);
        else if (op === 'stop') this.stop();
        else if (op === 'browser') void this.useBrowser();
    }

    private async useBrowser(): Promise<void> {
        if (!browserPossible()) return this.changed();
        try {
            this.access = await navigator.requestMIDIAccess({ sysex: false });
        } catch {
            this.state = 'off';
            this.why = this.words.denied;
            return this.changed();
        }
        this.access.onstatechange = () => this.ports();
        this.ports();
    }

    private ports(): void {
        if (!this.access) return;
        this.inputs = [...this.access.inputs.values()].map((port) => ({ id: port.id, name: port.name ?? port.id }));
        this.outputs = [...this.access.outputs.values()].map((port) => ({ id: port.id, name: port.name ?? port.id }));
        this.input = this.inputs.find((port) => port.id === this.input?.id) ?? this.inputs[0] ?? null;
        this.output = this.outputs.find((port) => port.name === this.input?.name) ?? null;
        this.state = this.input ? 'ready' : 'nodevice';
        this.why = this.input ? null : this.words.noKeyboard;
        this.bind();
        this.changed();
    }

    private bind(): void {
        this.access?.inputs.forEach((port) => {
            port.onmidimessage =
                port.id === this.input?.id
                    ? (message) => {
                          const event = message.data ? eventOf(message.data, message.timeStamp) : null;
                          if (event) this.emit(event);
                      }
                    : null;
        });
    }

    private choose(id: string): void {
        const port = this.inputs.find((input) => input.id === id);
        if (!port) return;
        this.input = port;
        this.output = this.outputs.find((output) => output.name === port.name) ?? this.output;
        this.bind();
        this.changed();
    }

    private send(event: Partial<MidiEvent> & { type: MidiEvent['type'] }): void {
        const output = this.output ? this.access?.outputs.get(this.output.id) : undefined;
        if (!output) return;
        output.send(bytesOf(event));
        if (event.type === 'on' && event.note != null) this.held.add(event.note);
        if (event.type === 'off' && event.note != null) this.held.delete(event.note);
    }

    private play(notes: Array<[number, number, number, number]>): void {
        const output = this.output ? this.access?.outputs.get(this.output.id) : undefined;
        if (!output) return;
        this.stop();
        const start = performance.now() + 50;
        for (const [note, velocity, at, length] of notes) {
            output.send([0x90, note, velocity], start + at);
            output.send([0x80, note, 0], start + at + Math.max(1, length));
        }
    }

    private stop(): void {
        const output = this.output ? this.access?.outputs.get(this.output.id) : undefined;
        (output as (MIDIOutput & { clear?: () => void }) | undefined)?.clear?.();
        this.held.forEach((note) => output?.send([0x80, note, 0]));
        this.held.clear();
        output?.send([0xb0, 123, 0]);
    }

    /** Окошко закрыто: ноты отпустить, клавиатуру отпустить. */
    close(): void {
        if (!this.access) return;
        this.stop();
        this.access.inputs.forEach((port) => {
            port.onmidimessage = null;
        });
        this.access.onstatechange = null;
        this.access = null;
    }
}
