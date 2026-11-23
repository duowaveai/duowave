// The DuoWave mascot (mascot/README.md): an engine any character runs on, and the characters. `createMascot` puts the
// current character (characters/index.ts CURRENT) or the one a host names into a container: the light body at once, the
// rich body when it is wanted and its chunk has loaded. The host keeps calling the same handle whoever is drawn.

import { CURRENT } from './characters/index.ts';
import { mount } from './engine/mount.ts';
import type { Mascot, MascotOptions } from './engine/types.ts';

export function createMascot(container: HTMLElement, options: MascotOptions = {}): Mascot {
    return mount(container, options.character ?? CURRENT, options);
}

export * from './engine/index.ts';
export { CHARACTERS, CURRENT, characterById } from './characters/index.ts';
export type { CharacterId } from './characters/index.ts';
export { DEEVI_COLORS, deevi } from './characters/deevi/index.ts';
