// Проверки QR пульта (docs/plans/home-and-helpers.md, блок 1, этап 8): кодировщик (src/ui/qr.ts) и свой распознаватель
// (src/ui/qr-read.ts) по кругу — каждая длина текста, поворот, перспектива, шум, размытие, муар экрана, блики, малый
// контраст вблизи, зеркало, испорченные кодовые слова (сколько исправляет Рид — Соломон, и ни одного неверного текста
// сверх того) — и то, что кодировщик кладёт по стандарту. Картинки синтетические, с постоянным зерном случайности:
// каждый запуск тот же. node ≥ 23.6 запускает файл как есть:
//
//     node panel/scripts/qr-check.ts        (или pnpm --filter panel qr)

import { BLOCKS, EC_PER_BLOCK, TOTAL, alignCentres, blockSizes, encodeQr, formatSpots, formatWord, functionMap, maskAt, qrPath, versionSpot, versionWord, zigzag } from '../src/ui/qr.ts';
import type { QrCode } from '../src/ui/qr.ts';
import { decodeQr } from '../src/ui/qr-read.ts';

interface Picture {
    data: Uint8ClampedArray;
    width: number;
    height: number;
}

/** Grey levels 0–255 as floats: every distortion works on these. */
interface Grey {
    v: Float32Array;
    width: number;
    height: number;
}

/** A seeded random source (mulberry32): the same pictures every run. */
function random(seed: number): () => number {
    return () => {
        seed = (seed + 0x6d2b79f5) | 0;
        let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

const bytesOf = (text: string): number => new TextEncoder().encode(text).length;
const ASCII = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_.:/#?&=';
const CYRILLIC = 'абвгдеёжзийклмнопрстуфхцчшщъыьэюяАБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ';

/** A text of exactly `bytes` UTF-8 bytes: ASCII, or Cyrillic (two bytes a letter, one ASCII letter when odd). */
function textOf(bytes: number, cyrillic: boolean, rnd: () => number): string {
    const pick = (set: string): string => set[Math.floor(rnd() * set.length)];
    if (!cyrillic) return Array.from({ length: bytes }, () => pick(ASCII)).join('');
    const letters = Array.from({ length: Math.floor(bytes / 2) }, () => pick(CYRILLIC));
    if (bytes % 2) letters.splice(Math.floor(rnd() * (letters.length + 1)), 0, pick(ASCII));
    return letters.join('');
}

function grey(width: number, height: number, value = 255): Grey {
    return { v: new Float32Array(width * height).fill(value), width, height };
}

/** The code drawn black on white, `scale` pixels a module, a quiet zone of `quiet` modules. */
function draw(code: QrCode, scale: number, quiet = 4): Grey {
    const side = (code.size + 2 * quiet) * scale;
    const g = grey(side, side);
    for (let y = 0; y < code.size; y++)
        for (let x = 0; x < code.size; x++)
            if (code.modules[y * code.size + x])
                for (let dy = 0; dy < scale; dy++) g.v.fill(0, ((y + quiet) * scale + dy) * side + (x + quiet) * scale, ((y + quiet) * scale + dy) * side + (x + quiet + 1) * scale);
    return g;
}

function picture(g: Grey): Picture {
    const data = new Uint8ClampedArray(g.width * g.height * 4);
    for (let i = 0; i < g.v.length; i++) {
        data[4 * i] = data[4 * i + 1] = data[4 * i + 2] = g.v[i];
        data[4 * i + 3] = 255;
    }
    return { data, width: g.width, height: g.height };
}

/** The 3×3 projective map taking four points to four others (Gaussian elimination, independent of the reader's). */
function homography(from: number[][], to: number[][]): number[] {
    const rows: number[][] = [];
    for (let i = 0; i < 4; i++) {
        const [x, y] = from[i];
        const [u, v] = to[i];
        rows.push([x, y, 1, 0, 0, 0, -u * x, -u * y, u], [0, 0, 0, x, y, 1, -v * x, -v * y, v]);
    }
    for (let c = 0; c < 8; c++) {
        let pivot = c;
        for (let r = c + 1; r < 8; r++) if (Math.abs(rows[r][c]) > Math.abs(rows[pivot][c])) pivot = r;
        [rows[c], rows[pivot]] = [rows[pivot], rows[c]];
        for (let r = 0; r < 8; r++) {
            if (r === c) continue;
            const f = rows[r][c] / rows[c][c];
            for (let k = c; k < 9; k++) rows[r][k] -= f * rows[c][k];
        }
    }
    return [...rows.map((row, i) => row[8] / row[i]), 1];
}

/** The picture seen through a projective map: its corners go to `corners` (clockwise from the top-left). */
function warp(g: Grey, corners: number[][], width: number, height: number, background = 255): Grey {
    const back = homography(corners, [[0, 0], [g.width, 0], [g.width, g.height], [0, g.height]]);
    const out = grey(width, height, background);
    for (let y = 0; y < height; y++)
        for (let x = 0; x < width; x++) {
            const w = back[6] * (x + 0.5) + back[7] * (y + 0.5) + back[8];
            const sx = (back[0] * (x + 0.5) + back[1] * (y + 0.5) + back[2]) / w - 0.5;
            const sy = (back[3] * (x + 0.5) + back[4] * (y + 0.5) + back[5]) / w - 0.5;
            if (sx < 0 || sy < 0 || sx > g.width - 1 || sy > g.height - 1) continue;
            const [x0, y0] = [Math.floor(sx), Math.floor(sy)];
            const [x1, y1] = [Math.min(x0 + 1, g.width - 1), Math.min(y0 + 1, g.height - 1)];
            const [fx, fy] = [sx - x0, sy - y0];
            const top = g.v[y0 * g.width + x0] * (1 - fx) + g.v[y0 * g.width + x1] * fx;
            const bottom = g.v[y1 * g.width + x0] * (1 - fx) + g.v[y1 * g.width + x1] * fx;
            out.v[y * width + x] = top * (1 - fy) + bottom * fy;
        }
    return out;
}

/** Turned by `degrees` round its centre, on a canvas large enough, `pad` pixels of white round it. */
function rotate(g: Grey, degrees: number, pad = 10): Grey {
    const a = (degrees * Math.PI) / 180;
    const [c, s] = [Math.cos(a), Math.sin(a)];
    const corners = [[0, 0], [g.width, 0], [g.width, g.height], [0, g.height]].map(([x, y]) => {
        const [dx, dy] = [x - g.width / 2, y - g.height / 2];
        return [dx * c - dy * s, dx * s + dy * c];
    });
    const minX = Math.min(...corners.map((p) => p[0]));
    const minY = Math.min(...corners.map((p) => p[1]));
    const width = Math.ceil(Math.max(...corners.map((p) => p[0])) - minX) + 2 * pad;
    const height = Math.ceil(Math.max(...corners.map((p) => p[1])) - minY) + 2 * pad;
    return warp(g, corners.map(([x, y]) => [x - minX + pad, y - minY + pad]), width, height);
}

/** A mild perspective: each corner moved inward by up to `amount` of the side, as when a phone is held at a slant. */
function slant(g: Grey, amount: number, rnd: () => number, pad = 12): Grey {
    const side = Math.max(g.width, g.height);
    const shift = (): number => rnd() * amount * side;
    const corners = [[shift(), shift()], [g.width - shift(), shift()], [g.width - shift(), g.height - shift()], [shift(), g.height - shift()]];
    return warp(g, corners.map(([x, y]) => [x + pad, y + pad]), g.width + 2 * pad, g.height + 2 * pad);
}

/** Gaussian noise of deviation `sigma` (Box–Muller). */
function noise(g: Grey, sigma: number, rnd: () => number): Grey {
    const v = g.v.map((value) => value + sigma * Math.sqrt(-2 * Math.log(1 - rnd())) * Math.cos(2 * Math.PI * rnd()));
    return { ...g, v };
}

/** A box blur of the given radius: a camera out of focus. */
function blur(g: Grey, radius: number): Grey {
    const once = (from: Float32Array, along: number, across: number, step: number, line: number): Float32Array => {
        const out = new Float32Array(from.length);
        for (let l = 0; l < across; l++)
            for (let i = 0; i < along; i++) {
                let total = 0;
                for (let k = -radius; k <= radius; k++) total += from[l * line + Math.min(Math.max(i + k, 0), along - 1) * step];
                out[l * line + i * step] = total / (2 * radius + 1);
            }
        return out;
    };
    return { ...g, v: once(once(g.v, g.width, g.height, 1, g.width), g.height, g.width, g.width, 1) };
}

/** A screen photographed: its pixel grid (dark gaps every `pitch` pixels) and interference bands (moiré). */
function screen(g: Grey, pitch: number, depth: number, bands: number, rnd: () => number): Grey {
    const angle = rnd() * Math.PI;
    const phase = rnd() * 2 * Math.PI;
    const v = g.v.map((value, i) => {
        const [x, y] = [i % g.width, Math.floor(i / g.width)];
        const grid = x % pitch === pitch - 1 || y % pitch === pitch - 1 ? 0.7 : 1;
        const band = 1 - depth * (0.5 + 0.5 * Math.sin((2 * Math.PI * (x * Math.cos(angle) + y * Math.sin(angle))) / bands + phase));
        return value * grid * band;
    });
    return { ...g, v };
}

/** Glare and uneven light: black lifted to `low`, white down to `high`, a falloff across the picture. */
function light(g: Grey, low: number, high: number, falloff: number): Grey {
    const v = g.v.map((value, i) => {
        const x = (i % g.width) / g.width;
        const y = Math.floor(i / g.width) / g.height;
        return (low + ((high - low) * value) / 255) * (1 - falloff * (0.6 * x + 0.4 * y));
    });
    return { ...g, v };
}

/** The picture in the middle of a larger frame of grey `background`: a screen's bezel, a wall. */
function framed(g: Grey, width: number, height: number, background: number): Grey {
    const out = grey(width, height, background);
    const [x, y] = [Math.floor((width - g.width) / 2), Math.floor((height - g.height) / 2)];
    for (let row = 0; row < g.height; row++) out.v.set(g.v.subarray(row * g.width, (row + 1) * g.width), (row + y) * width + x);
    return out;
}

function mirror(g: Grey): Grey {
    const v = new Float32Array(g.v.length);
    for (let y = 0; y < g.height; y++) for (let x = 0; x < g.width; x++) v[y * g.width + x] = g.v[y * g.width + g.width - 1 - x];
    return { ...g, v };
}

const versionOf = (code: QrCode): number => (code.size - 17) / 4;

/** Where each codeword of each block lies in the placed stream: data interleaved, then error correction. */
function streamIndex(version: number): number[][] {
    const sizes = blockSizes(version);
    const ec = EC_PER_BLOCK[version - 1];
    const index = sizes.map((n) => new Array<number>(n + ec));
    let k = 0;
    for (let i = 0; i < Math.max(...sizes); i++) sizes.forEach((n, b) => i < n && (index[b][i] = k++));
    for (let i = 0; i < ec; i++) sizes.forEach((n, b) => (index[b][n + i] = k++));
    return index;
}

/** A copy of the code with `count` codewords of every block spoiled (each XORed with a random non-zero byte). */
function spoil(code: QrCode, count: number, rnd: () => number): QrCode {
    const version = versionOf(code);
    const order = zigzag(code.size, functionMap(version));
    const modules = code.modules.slice();
    for (const block of streamIndex(version)) {
        const chosen = new Set<number>();
        while (chosen.size < count) chosen.add(block[Math.floor(rnd() * block.length)]);
        for (const k of chosen) {
            const flip = 1 + Math.floor(rnd() * 255);
            for (let bit = 0; bit < 8; bit++) if ((flip >>> (7 - bit)) & 1) modules[order[8 * k + bit]] ^= 1;
        }
    }
    return { size: code.size, modules };
}

/** The four penalties of ISO/IEC 18004 7.8.3, written apart from the encoder's: runs, 2×2, finder-like, balance. */
function penalties(m: Uint8Array, size: number): number {
    const get = (x: number, y: number): number => m[y * size + x];
    let total = 0;
    const lines: number[][] = [];
    for (let i = 0; i < size; i++) {
        lines.push(Array.from({ length: size }, (_, k) => get(k, i)));
        lines.push(Array.from({ length: size }, (_, k) => get(i, k)));
    }
    for (const line of lines) {
        const text = line.join('');
        for (const run of text.match(/0{5,}|1{5,}/g) ?? []) total += 3 + run.length - 5;
        for (const pattern of ['10111010000', '00001011101']) for (let at = text.indexOf(pattern); at >= 0; at = text.indexOf(pattern, at + 1)) total += 40;
    }
    let dark = 0;
    for (let y = 0; y < size; y++)
        for (let x = 0; x < size; x++) {
            dark += get(x, y);
            if (x + 1 < size && y + 1 < size && get(x, y) === get(x + 1, y) && get(x, y) === get(x, y + 1) && get(x, y) === get(x + 1, y + 1)) total += 3;
        }
    return total + 10 * Math.floor(Math.abs((100 * dark) / (size * size) - 50) / 5);
}

// ─── The checks ──────────────────────────────────────────────────────────────────────────────────────────────────

const checks: Array<[string, () => void]> = [];
const check = (name: string, run: () => void): void => void checks.push([name, run]);
function expect(condition: unknown, message: string): asserts condition {
    if (!condition) throw new Error(message);
}

check('every_length_from_1_to_213_bytes_encodes_and_reads_back', () => {
    const rnd = random(1);
    for (let bytes = 1; bytes <= 213; bytes++)
        for (const cyrillic of [false, true]) {
            const text = textOf(bytes, cyrillic, rnd);
            expect(bytesOf(text) === bytes, `a text of ${bytes} bytes was made ${bytesOf(text)} long`);
            const code = encodeQr(text);
            expect(code, `${bytes} bytes do not encode`);
            const scale = 4 + (bytes % 5);
            const read = decodeQr(picture(draw(code, scale)));
            expect(read === text, `${bytes} bytes (${cyrillic ? 'Cyrillic' : 'ASCII'}, ${scale} px a module) read back as ${JSON.stringify(read)}`);
        }
});

check('the_decoder_reads_through_rotation_perspective_noise_and_flipped_modules', () => {
    const rnd = random(2);
    for (const bytes of [12, 40, 100, 160, 210]) {
        const text = textOf(bytes, bytes % 2 === 0, rnd);
        const code = encodeQr(text) as QrCode;
        const t = Math.floor(EC_PER_BLOCK[versionOf(code) - 1] / 2);
        for (let degrees = 0; degrees < 360; degrees += 15) {
            const seen = (spoiled: QrCode): string | null => decodeQr(picture(noise(slant(rotate(draw(spoiled, 6), degrees), 0.05, rnd), 18, rnd)));
            const fixable = seen(spoil(code, t - 1, rnd));
            expect(fixable === text, `${bytes} bytes at ${degrees}° with ${t - 1} spoiled codewords a block read as ${JSON.stringify(fixable)}`);
            const edge = seen(spoil(code, t, rnd));
            expect(edge === null || edge === text, `${bytes} bytes at ${degrees}° with ${t} spoiled codewords a block read a wrong text ${JSON.stringify(edge)}`);
        }
    }
});

check('reed_solomon_corrects_as_many_spoiled_codewords_as_the_standard_allows_and_never_reads_a_wrong_text', () => {
    // On a clean picture: ⌊ec/2⌋ spoiled codewords in every block — all a block can correct — always read; one more is
    // past correcting, and then the decoder says nothing rather than a wrong text.
    const rnd = random(8);
    for (const bytes of [12, 40, 100, 160, 210]) {
        const text = textOf(bytes, bytes % 2 === 1, rnd);
        const code = encodeQr(text) as QrCode;
        const t = Math.floor(EC_PER_BLOCK[versionOf(code) - 1] / 2);
        for (let i = 0; i < 20; i++) {
            const most = decodeQr(picture(draw(spoil(code, t, rnd), 4)));
            expect(most === text, `${bytes} bytes with ${t} spoiled codewords a block read as ${JSON.stringify(most)}`);
            const over = decodeQr(picture(draw(spoil(code, t + 1, rnd), 4)));
            expect(over === null || over === text, `${bytes} bytes with ${t + 1} spoiled codewords a block read a wrong text ${JSON.stringify(over)}`);
        }
    }
});

check('a_close_up_of_little_contrast_reads_on_a_light_and_on_a_dark_surround', () => {
    // A lamp's glare on a laptop's screen held close: black lifted to 110–120, white down to 180–190, a module 12–14 px
    // (the code about half the frame), a light page or a dark bezel round it. The local means alone find no finders here.
    const rnd = random(9);
    const text = 'https://192.0.2.10:7781 ABCDE-FGHJK-MNPQR-STVWX-YZ012';
    const code = encodeQr(text) as QrCode;
    for (const [low, high] of [[110, 180], [120, 190]])
        for (const scale of [12, 14])
            for (const surround of [high, 40]) {
                const got = decodeQr(picture(noise(framed(light(draw(code, scale), low, high, 0), 1024, 768, surround), 4, rnd)));
                expect(got === text, `${low}–${high} at ${scale} px a module, surround ${surround}: ${JSON.stringify(got)}`);
            }
});

check('the_decoder_reads_a_photo_of_a_screen_with_its_pixel_grid_moire_glare_blur_and_uneven_light', () => {
    const rnd = random(3);
    const text = 'https://192.0.2.10:7781 ABCDE-FGHJK-MNPQR-STVWX-YZ012';
    const code = encodeQr(text) as QrCode;
    let read = 0;
    const tries = 40;
    for (let i = 0; i < tries; i++) {
        const scale = 5 + (i % 4);
        let g = draw(code, scale, 6);
        g = screen(g, 3 + (i % 3), 0.18, 9 + (i % 7) * 3, rnd);
        g = light(g, 35 + (i % 5) * 8, 225 - (i % 4) * 10, 0.35);
        g = slant(rotate(g, (i * 37) % 360), 0.08, rnd);
        g = blur(g, 1 + (i % 2));
        g = noise(g, 10, rnd);
        const got = decodeQr(picture(g));
        expect(got === null || got === text, `screen photo ${i} read a wrong text ${JSON.stringify(got)}`);
        if (got === text) read++;
    }
    expect(read === tries, `${read} of ${tries} screen photos read`);
});

check('small_modules_and_a_strong_blur_still_read', () => {
    const rnd = random(4);
    for (const bytes of [20, 55, 120]) {
        const text = textOf(bytes, false, rnd);
        const code = encodeQr(text) as QrCode;
        for (const [scale, radius] of [[3, 0], [3, 1], [4, 1], [6, 2], [8, 3]]) {
            const got = decodeQr(picture(radius ? blur(draw(code, scale), radius) : draw(code, scale)));
            expect(got === text, `${bytes} bytes at ${scale} px a module, blur ${radius}: ${JSON.stringify(got)}`);
        }
    }
});

check('a_mirrored_picture_reads_too', () => {
    const rnd = random(5);
    for (const bytes of [9, 50, 150]) {
        const text = textOf(bytes, true, rnd);
        const got = decodeQr(picture(mirror(rotate(draw(encodeQr(text) as QrCode, 5), 20))));
        expect(got === text, `a mirrored code of ${bytes} bytes read as ${JSON.stringify(got)}`);
    }
});

check('a_picture_without_a_code_reads_nothing', () => {
    const rnd = random(6);
    expect(decodeQr(picture(grey(300, 200))) === null, 'a blank picture read something');
    expect(decodeQr(picture(noise(grey(320, 240, 128), 60, rnd))) === null, 'noise read something');
    // Finders and timing without valid data: the format bits or Reed–Solomon refuse it.
    const code = encodeQr('https://192.0.2.10:7781 ABCDE-FGHJK-MNPQR-STVWX-YZ012') as QrCode;
    const junk = code.modules.slice();
    const map = functionMap(versionOf(code));
    for (let i = 0; i < junk.length; i++) if (!map[i]) junk[i] = rnd() < 0.5 ? 1 : 0;
    expect(decodeQr(picture(draw({ size: code.size, modules: junk }, 6))) === null, 'random data behind real finders read something');
    expect(decodeQr({ data: new Uint8ClampedArray(4 * 10 * 10), width: 10, height: 10 }) === null, 'a tiny picture read something');
});

check('the_smallest_version_is_chosen', () => {
    const limits = [14, 26, 42, 62, 84, 106, 122, 152, 180, 213];
    limits.forEach((bytes, i) => {
        expect(versionOf(encodeQr('a'.repeat(bytes)) as QrCode) === i + 1, `${bytes} bytes are not version ${i + 1}`);
        const more = encodeQr('a'.repeat(bytes + 1));
        if (i < 9) expect(more && versionOf(more) === i + 2, `${bytes + 1} bytes are not version ${i + 2}`);
        else expect(more === null, `${bytes + 1} bytes should not fit version 10`);
    });
});

check('the_mask_has_the_lowest_penalty', () => {
    const rnd = random(7);
    for (const bytes of [5, 14, 33, 61, 90, 133, 200]) {
        const code = encodeQr(textOf(bytes, false, rnd)) as QrCode;
        const { size } = code;
        const version = versionOf(code);
        const map = functionMap(version);
        let word = 0;
        for (let i = 0; i < 15; i++) {
            const [x, y] = formatSpots(size, i);
            word |= code.modules[y * size + x] << i;
        }
        const chosen = [0, 1, 2, 3, 4, 5, 6, 7].find((mask) => formatWord(0, mask) === word);
        expect(chosen !== undefined, `no level-M format word in a code of ${bytes} bytes`);
        const scores = [0, 1, 2, 3, 4, 5, 6, 7].map((mask) => {
            const m = code.modules.slice();
            for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) if (!map[y * size + x]) m[y * size + x] ^= maskAt(chosen, x, y) ^ maskAt(mask, x, y);
            for (let i = 0; i < 15; i++) {
                const [ax, ay, bx, by] = formatSpots(size, i);
                m[ay * size + ax] = m[by * size + bx] = (formatWord(0, mask) >>> i) & 1;
            }
            return penalties(m, size);
        });
        expect(scores[chosen] === Math.min(...scores), `${bytes} bytes: mask ${chosen} scores ${scores[chosen]}, the lowest is ${Math.min(...scores)}`);
    }
});

check('finders_timing_and_dark_module_are_where_the_standard_puts_them', () => {
    for (let version = 1; version <= 10; version++) {
        const bytes = [10, 20, 35, 50, 70, 90, 110, 140, 170, 200][version - 1];
        const code = encodeQr('x'.repeat(bytes)) as QrCode;
        const { size, modules } = code;
        expect(size === 17 + 4 * version, `version ${version} is ${size} modules`);
        const get = (x: number, y: number): number => modules[y * size + x];
        for (const [cx, cy] of [[3, 3], [size - 4, 3], [3, size - 4]])
            for (let dy = -4; dy <= 4; dy++)
                for (let dx = -4; dx <= 4; dx++) {
                    const [x, y] = [cx + dx, cy + dy];
                    if (x < 0 || y < 0 || x >= size || y >= size) continue;
                    const ring = Math.max(Math.abs(dx), Math.abs(dy));
                    expect(get(x, y) === (ring === 2 || ring === 4 ? 0 : 1), `version ${version}: finder at ${cx},${cy} wrong at ${x},${y}`);
                }
        for (let i = 8; i < size - 8; i++) {
            expect(get(i, 6) === (i % 2 === 0 ? 1 : 0) && get(6, i) === (i % 2 === 0 ? 1 : 0), `version ${version}: timing wrong at ${i}`);
        }
        expect(get(8, size - 8) === 1, `version ${version}: no dark module`);
        for (const [cx, cy] of alignCentres(version))
            for (let dy = -2; dy <= 2; dy++)
                for (let dx = -2; dx <= 2; dx++)
                    expect(get(cx + dx, cy + dy) === (Math.max(Math.abs(dx), Math.abs(dy)) === 1 ? 0 : 1), `version ${version}: alignment at ${cx},${cy} wrong`);
        if (version >= 7)
            for (let i = 0; i < 18; i++) {
                const [x, y] = versionSpot(size, i);
                const bit = (versionWord(version) >>> i) & 1;
                expect(get(x, y) === bit && get(y, x) === bit, `version ${version}: version bit ${i} wrong`);
            }
        expect(TOTAL[version - 1] * 8 <= zigzag(size, functionMap(version)).length, `version ${version}: fewer data modules than codeword bits`);
        expect(BLOCKS[version - 1] === blockSizes(version).length, `version ${version}: block count`);
    }
});

check('a_connection_text_fits', () => {
    const text = 'https://192.0.2.200:65535 ABCDE-FGHJK-MNPQR-STVWX-YZ012';
    const code = encodeQr(text) as QrCode;
    expect(versionOf(code) <= 5, `the connection text needs version ${versionOf(code)}`);
    expect(decodeQr(picture(draw(code, 6))) === text, 'the connection text does not read back');
});

check('the_svg_path_draws_every_dark_module_once', () => {
    const code = encodeQr('https://192.0.2.10:7781 ABCDE-FGHJK-MNPQR-STVWX-YZ012') as QrCode;
    const drawn = new Uint8Array(code.size * code.size);
    for (const [, x, y, run] of qrPath(code, 4).matchAll(/M(\d+) (\d+)h(\d+)v1h-\d+z/g))
        for (let i = 0; i < Number(run); i++) drawn[(Number(y) - 4) * code.size + Number(x) - 4 + i] += 1;
    expect(drawn.every((n, i) => n === code.modules[i]), 'the path does not draw exactly the dark modules');
});

let failed = 0;
for (const [name, run] of checks) {
    const started = performance.now();
    try {
        run();
        console.log(`ok   ${name} (${Math.round(performance.now() - started)} ms)`);
    } catch (error) {
        failed++;
        console.log(`FAIL ${name}: ${(error as Error).message}`);
    }
}
console.log(failed ? `${failed} of ${checks.length} failed` : `all ${checks.length} passed`);
process.exitCode = failed ? 1 : 0;
