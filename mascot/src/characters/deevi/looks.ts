// Deevi's colours and sounds: the logo's lime body and dark ink, a colour and a chirp per mood (the 3D handoff,
// 23.11.2026). Open question: `think` is violet, while in the panel violet means «an agent answers» (DESIGN.md,
// Who-Answers Rule).

import type { MoodId } from '../../engine/moods.ts';
import type { MoodLook, Tunes } from '../../engine/types.ts';
import { DIVI_COLORS } from './mark.ts';

/** The logo's colours (mark.ts): the lime body and the dark ink of the eyes. */
export const DEEVI_COLORS = { body: DIVI_COLORS.body, ink: DIVI_COLORS.eye } as const;

/** Deevi has a look for every mood, so nothing falls back. */
export const LOOKS: Record<MoodId, MoodLook> = {
    idle: { color: DEEVI_COLORS.body },
    tender: { color: DEEVI_COLORS.body },
    wink: { color: DEEVI_COLORS.body },
    sleep: { color: '#A3C777' },
    listen: { color: '#C2F77C' },
    think: { color: '#9D90FF' },
    speak: { color: DEEVI_COLORS.body },
    success: { color: '#6FE894' },
    surprise: { color: '#E6F56B' },
    sad: { color: '#AEBFB3' },
};

/** Cheeks when he blushes (`tender`). */
export const CHEEK = '#FF7F96';

export const TUNES: Tunes = {
    idle: [660],
    tender: [659, 784, 659, 988],
    wink: [784, 1047],
    sleep: [392, 294],
    listen: [523, 784],
    think: [440, 494, 440],
    speak: [587, 659, 587, 740],
    success: [523, 659, 784, 1047],
    surprise: [440, 1175],
    sad: [440, 370, 311],
    tap: [740, 990],
    wake: [523, 784],
};
