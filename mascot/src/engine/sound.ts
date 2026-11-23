// Chirps: a character's tune is a few notes in Hz, played one every 90 ms as short sine blips. The engine plays them
// only when the host turned sound on (browsers allow sound only after a user gesture).

/** Notes in Hz, one every 90 ms. */
export type Tune = readonly number[];

/** Play `notes` on `ctx` (created on the first call). Returns the context to keep for the next call. */
export function playTune(ctx: AudioContext | null, notes: Tune): AudioContext | null {
    try {
        const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
        if (!Ctor) return ctx;
        const audio = ctx ?? new Ctor();
        const t = audio.currentTime;
        notes.forEach((frequency, index) => {
            const osc = audio.createOscillator();
            const gain = audio.createGain();
            const at = t + index * 0.09;
            osc.type = 'sine';
            osc.frequency.value = frequency;
            gain.gain.setValueAtTime(0, at);
            gain.gain.linearRampToValueAtTime(0.12, at + 0.015);
            gain.gain.exponentialRampToValueAtTime(0.001, at + 0.16);
            osc.connect(gain);
            gain.connect(audio.destination);
            osc.start(at);
            osc.stop(at + 0.18);
        });
        return audio;
    } catch {
        return ctx;
    }
}
