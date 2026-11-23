// The characters this library ships, and the one DuoWave shows now.
//
// To put someone else in Deevi's place everywhere — the panel, the app, the sites — add them under characters/<id>/,
// list them in CHARACTERS and point CURRENT at them. Hosts that name no character follow CURRENT; none of them changes.

import type { Character } from '../engine/types.ts';
import { deevi } from './deevi/index.ts';

export const CHARACTERS = { deevi } as const satisfies Record<string, Character>;

export type CharacterId = keyof typeof CHARACTERS;

/** The character DuoWave shows wherever a host doesn't name one. */
export const CURRENT: Character = CHARACTERS.deevi;

/** A character by id; CURRENT for an unknown one. */
export const characterById = (id: string | undefined): Character => (id && id in CHARACTERS ? CHARACTERS[id as CharacterId] : CURRENT);
