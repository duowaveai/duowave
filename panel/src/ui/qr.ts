// QR-код «Подключить шлюз» (docs/plans/home-and-helpers.md, блок 1): кодировщик для листа ключа подключения и таблицы,
// которые с ним делит распознаватель (qr-read.ts). По ISO/IEC 18004, без зависимостей: байтовый режим, уровень
// коррекции M, версии 1–10. Только стираемый TypeScript (без enum), чтобы node запускал файл как есть
// (panel/scripts/qr-check.ts).

/** A QR code for a short text: byte mode, error correction M, versions 1–10 (up to 213 bytes of UTF-8). */
export interface QrCode {
    /** Modules a side: 21 for version 1 … 57 for version 10. */
    size: number;
    /** `modules[y * size + x]`: 1 dark, 0 light. */
    modules: Uint8Array;
}

/** GF(256) with the primitive polynomial 0x11D: `EXP` runs twice round, so a product of two logs needs no modulo. */
export const EXP = new Uint8Array(512);
export const LOG = new Uint8Array(256);
for (let i = 0, x = 1; i < 255; i++) {
    EXP[i] = EXP[i + 255] = x;
    LOG[x] = i;
    x = x & 0x80 ? (x << 1) ^ 0x11d : x << 1;
}

export const mul = (a: number, b: number): number => (a && b ? EXP[LOG[a] + LOG[b]] : 0);

/** Error correction M, index = version − 1: all codewords, error-correction codewords per block, blocks. */
export const TOTAL = [26, 44, 70, 100, 134, 172, 196, 242, 292, 346];
export const EC_PER_BLOCK = [10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
export const BLOCKS = [1, 1, 1, 2, 2, 4, 4, 4, 5, 5];
/** Alignment pattern centres of versions 2–10 (index = version − 2), the same list for rows and columns. */
export const ALIGN = [[6, 18], [6, 22], [6, 26], [6, 30], [6, 34], [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50]];

/** Data codewords of a version at level M. */
export const dataOf = (version: number): number => TOTAL[version - 1] - EC_PER_BLOCK[version - 1] * BLOCKS[version - 1];

/** Data codewords of each block: the short ones first, the last `data mod blocks` one longer. */
export function blockSizes(version: number): number[] {
    const blocks = BLOCKS[version - 1];
    const data = dataOf(version);
    const short = Math.floor(data / blocks);
    return Array.from({ length: blocks }, (_, b) => (b >= blocks - (data % blocks) ? short + 1 : short));
}

/** Alignment centres that are drawn: every pair of the list but the three under the finders. */
export function alignCentres(version: number): Array<[number, number]> {
    const list = version >= 2 ? ALIGN[version - 2] : [];
    const last = list[list.length - 1];
    const out: Array<[number, number]> = [];
    for (const y of list) for (const x of list) if (!((x === 6 && y === 6) || (x === 6 && y === last) || (x === last && y === 6))) out.push([x, y]);
    return out;
}

/**
 * Function modules of a version — finders with their separators, the format areas, timing, alignment, the dark
 * module and, from version 7, the version blocks: 1 where no data goes.
 */
export function functionMap(version: number): Uint8Array {
    const size = 17 + 4 * version;
    const map = new Uint8Array(size * size);
    const mark = (x0: number, y0: number, w: number, h: number): void => {
        for (let y = y0; y < y0 + h; y++) for (let x = x0; x < x0 + w; x++) map[y * size + x] = 1;
    };
    mark(0, 0, 9, 9);
    mark(size - 8, 0, 8, 9);
    mark(0, size - 8, 9, 8);
    mark(6, 0, 1, size);
    mark(0, 6, size, 1);
    for (const [x, y] of alignCentres(version)) mark(x - 2, y - 2, 5, 5);
    if (version >= 7) {
        mark(size - 11, 0, 3, 6);
        mark(0, size - 11, 6, 3);
    }
    return map;
}

/**
 * Data modules in the order codeword bits fill them, as indices `y * size + x`: two-column strips from the bottom
 * right, up then down, the timing column skipped; the remainder modules come last.
 */
export function zigzag(size: number, map: Uint8Array): Int32Array {
    const order: number[] = [];
    for (let right = size - 1; right >= 1; right -= 2) {
        if (right === 6) right = 5;
        const upward = ((right + 1) & 2) === 0;
        for (let step = 0; step < size; step++) {
            const y = upward ? size - 1 - step : step;
            for (let x = right; x >= right - 1; x--) if (!map[y * size + x]) order.push(y * size + x);
        }
    }
    return Int32Array.from(order);
}

/** The eight data masks: 1 where the module is inverted. */
export function maskAt(mask: number, x: number, y: number): number {
    let flip: boolean;
    switch (mask) {
        case 0: flip = (x + y) % 2 === 0; break;
        case 1: flip = y % 2 === 0; break;
        case 2: flip = x % 3 === 0; break;
        case 3: flip = (x + y) % 3 === 0; break;
        case 4: flip = (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0; break;
        case 5: flip = ((x * y) % 2) + ((x * y) % 3) === 0; break;
        case 6: flip = (((x * y) % 2) + ((x * y) % 3)) % 2 === 0; break;
        default: flip = (((x + y) % 2) + ((x * y) % 3)) % 2 === 0;
    }
    return flip ? 1 : 0;
}

/** Remainder of a BCH code: `value` shifted by the generator's degree, divided by the generator. */
function bch(value: number, generator: number, degree: number): number {
    let rem = value << degree;
    for (let bit = 31 - Math.clz32(rem); bit >= degree; bit--) if ((rem >>> bit) & 1) rem ^= generator << (bit - degree);
    return rem;
}

/** The 15 format bits of a level (L 1, M 0, Q 3, H 2) and a mask: BCH(15,5), then XOR 0x5412. */
export function formatWord(level: number, mask: number): number {
    const data = (level << 3) | mask;
    return ((data << 10) | bch(data, 0x537, 10)) ^ 0x5412;
}

/** The 18 version bits of versions 7 and up: BCH(18,6). */
export function versionWord(version: number): number {
    return (version << 12) | bch(version, 0x1f25, 12);
}

/** Where format bit `i` (0 = least significant) lies: [x, y] in the copy round the top-left finder, then the other. */
export function formatSpots(size: number, i: number): [number, number, number, number] {
    const [ax, ay] = i < 6 ? [8, i] : i < 8 ? [8, i + 1] : i === 8 ? [7, 8] : [14 - i, 8];
    const [bx, by] = i < 8 ? [size - 1 - i, 8] : [8, size - 15 + i];
    return [ax, ay, bx, by];
}

/** Where version bit `i` lies: [x, y] in the block above the bottom-left finder; the other copy is mirrored. */
export function versionSpot(size: number, i: number): [number, number] {
    return [Math.floor(i / 3), size - 11 + (i % 3)];
}

/** Generator polynomial ∏ (x − αⁱ), i = 0…n−1, highest power first, the leading 1 left out. */
function generator(n: number): Uint8Array {
    let poly = [1];
    for (let i = 0; i < n; i++) {
        const next = new Array<number>(poly.length + 1).fill(0);
        poly.forEach((c, j) => {
            next[j] ^= c;
            next[j + 1] ^= mul(c, EXP[i]);
        });
        poly = next;
    }
    return Uint8Array.from(poly.slice(1));
}

/** Reed–Solomon error-correction codewords of one block. */
function correction(data: Uint8Array, n: number): Uint8Array {
    const gen = generator(n);
    const rem = new Uint8Array(n);
    for (const byte of data) {
        const factor = byte ^ rem[0];
        rem.copyWithin(0, 1);
        rem[n - 1] = 0;
        for (let i = 0; i < n; i++) rem[i] ^= mul(gen[i], factor);
    }
    return rem;
}

/** Penalty of a finished matrix (ISO/IEC 18004, 7.8.3): runs, 2×2 blocks, finder-like rows, the balance of dark. */
export function penalty(m: Uint8Array, size: number): number {
    let score = 0;
    let dark = 0;
    for (let line = 0; line < size; line++)
        for (const across of [true, false]) {
            const at = (i: number): number => (across ? m[line * size + i] : m[i * size + line]);
            let run = 1;
            for (let i = 1; i <= size; i++) {
                if (i < size && at(i) === at(i - 1)) run++;
                else {
                    if (run >= 5) score += run - 2;
                    run = 1;
                }
            }
            for (let i = 0; i + 11 <= size; i++) {
                let a = true;
                let b = true;
                for (let k = 0; k < 11; k++) {
                    a &&= at(i + k) === FINDER_A[k];
                    b &&= at(i + k) === FINDER_B[k];
                }
                score += (a ? 40 : 0) + (b ? 40 : 0);
            }
        }
    for (let y = 0; y < size; y++)
        for (let x = 0; x < size; x++) {
            const v = m[y * size + x];
            dark += v;
            if (x < size - 1 && y < size - 1 && v === m[y * size + x + 1] && v === m[(y + 1) * size + x] && v === m[(y + 1) * size + x + 1]) score += 3;
        }
    return score + Math.floor(Math.abs((dark * 100) / (size * size) - 50) / 5) * 10;
}

const FINDER_A = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
const FINDER_B = [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1];

/** The smallest version (1–10) that holds this many bytes at level M; null — none does. */
export function versionFor(bytes: number): number | null {
    for (let version = 1; version <= 10; version++) if (4 + (version < 10 ? 8 : 16) + 8 * bytes <= 8 * dataOf(version)) return version;
    return null;
}

/** A matrix with the function patterns drawn and the data placed, before any mask. */
function layout(version: number, codewords: Uint8Array): Uint8Array {
    const size = 17 + 4 * version;
    const m = new Uint8Array(size * size);
    const set = (x: number, y: number, dark: boolean | number): void => {
        m[y * size + x] = dark ? 1 : 0;
    };
    for (const [cx, cy] of [[3, 3], [size - 4, 3], [3, size - 4]])
        for (let dy = -4; dy <= 4; dy++)
            for (let dx = -4; dx <= 4; dx++) {
                const x = cx + dx;
                const y = cy + dy;
                const ring = Math.max(Math.abs(dx), Math.abs(dy));
                if (x >= 0 && y >= 0 && x < size && y < size) set(x, y, ring !== 2 && ring !== 4);
            }
    for (let i = 8; i < size - 8; i++) {
        set(6, i, i % 2 === 0);
        set(i, 6, i % 2 === 0);
    }
    for (const [cx, cy] of alignCentres(version))
        for (let dy = -2; dy <= 2; dy++) for (let dx = -2; dx <= 2; dx++) set(cx + dx, cy + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    set(8, size - 8, 1);
    if (version >= 7) {
        const word = versionWord(version);
        for (let i = 0; i < 18; i++) {
            const [x, y] = versionSpot(size, i);
            set(x, y, (word >>> i) & 1);
            set(y, x, (word >>> i) & 1);
        }
    }
    const order = zigzag(size, functionMap(version));
    for (let i = 0; i < codewords.length * 8; i++) m[order[i]] = (codewords[i >> 3] >>> (7 - (i & 7))) & 1;
    return m;
}

/** A copy of a laid-out matrix with a mask applied to the data and its format bits written. */
export function withMask(base: Uint8Array, version: number, mask: number): Uint8Array {
    const size = 17 + 4 * version;
    const map = functionMap(version);
    const m = base.slice();
    for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) if (!map[y * size + x]) m[y * size + x] ^= maskAt(mask, x, y);
    const word = formatWord(0, mask);
    for (let i = 0; i < 15; i++) {
        const [ax, ay, bx, by] = formatSpots(size, i);
        m[ay * size + ax] = m[by * size + bx] = (word >>> i) & 1;
    }
    return m;
}

/** The codewords in the order they are placed: data interleaved across blocks, then error correction. */
export function codewordsOf(version: number, data: Uint8Array): Uint8Array {
    const sizes = blockSizes(version);
    const ec = EC_PER_BLOCK[version - 1];
    const blocks: Uint8Array[] = [];
    let at = 0;
    for (const length of sizes) {
        blocks.push(data.subarray(at, at + length));
        at += length;
    }
    const checks = blocks.map((block) => correction(block, ec));
    const out: number[] = [];
    for (let i = 0; i < sizes[sizes.length - 1]; i++) blocks.forEach((block) => i < block.length && out.push(block[i]));
    for (let i = 0; i < ec; i++) checks.forEach((check) => out.push(check[i]));
    return Uint8Array.from(out);
}

/** The data codewords of a text: mode 0100, count, the bytes, a terminator, padding to a byte, then 0xEC 0x11. */
export function dataCodewords(bytes: Uint8Array, version: number): Uint8Array {
    const capacity = dataOf(version);
    const data = new Uint8Array(capacity);
    let bit = 0;
    const put = (value: number, length: number): void => {
        for (let i = length - 1; i >= 0; i--, bit++) data[bit >> 3] |= ((value >>> i) & 1) << (7 - (bit & 7));
    };
    put(4, 4);
    put(bytes.length, version < 10 ? 8 : 16);
    for (const byte of bytes) put(byte, 8);
    bit += Math.min(4, capacity * 8 - bit);
    for (let i = Math.ceil(bit / 8), pad = 0xec; i < capacity; i++, pad ^= 0xec ^ 0x11) data[i] = pad;
    return data;
}

export function encodeQr(text: string): QrCode | null {
    const bytes = new TextEncoder().encode(text);
    const version = versionFor(bytes.length);
    if (version === null) return null;
    const base = layout(version, codewordsOf(version, dataCodewords(bytes, version)));
    const size = 17 + 4 * version;
    let best: Uint8Array | null = null;
    let lowest = Infinity;
    for (let mask = 0; mask < 8; mask++) {
        const m = withMask(base, version, mask);
        const score = penalty(m, size);
        if (score < lowest) {
            lowest = score;
            best = m;
        }
    }
    return { size, modules: best as Uint8Array };
}

/** SVG path data: one `M x y h n v1 h-n z` per run of dark modules in a row, shifted by the quiet zone. */
export function qrPath(code: QrCode, quiet = 4): string {
    let path = '';
    for (let y = 0; y < code.size; y++)
        for (let x = 0; x < code.size; x++) {
            if (!code.modules[y * code.size + x]) continue;
            let end = x;
            while (end < code.size && code.modules[y * code.size + end]) end++;
            path += `M${x + quiet} ${y + quiet}h${end - x}v1h${x - end}z`;
            x = end;
        }
    return path;
}
