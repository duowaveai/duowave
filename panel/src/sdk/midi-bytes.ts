// Сообщения MIDI байтами и обратно — без шлюза, DOM и Preact: их читает канал пульта (midi.ts) и веб-хозяин сайта
// (web-host/midi.ts), где клавиатура — только через браузер.

import type { MidiEvent } from './midi';

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
export function bytesOf(event: Partial<MidiEvent> & { type: MidiEvent['type'] }): number[] {
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
