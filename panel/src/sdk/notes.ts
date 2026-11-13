// Ноты словами и мелодии дорожек (docs/plans/midi.md): чистые помощники без шлюза и без пульта — их берут и пульт
// (sdk/midi.ts), и окошко приложения (frame/sdk.tsx).

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

/** Мелодия дорожки: верхний голос уже взят шлюзом; здесь — по желанию сдвиг, чтобы начиналась с нуля. */
export function melodyOf(track: MidiTrack): Array<[number, number, number]> {
    const first = track.melody[0]?.[1] ?? 0;
    return track.melody.map(([note, start, length]) => [note, Math.round((start - first) * 1000) / 1000, length]);
}
