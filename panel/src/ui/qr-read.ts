// Свой распознаватель QR в пульте (решение владельца 15.11: «Сфотографировать QR» работает на любом телефоне, есть у
// браузера BarcodeDetector или нет): снимок листа «Подключить шлюз» с экрана компьютера или с бумаги → текст
// подключения. По ISO/IEC 18004, как кодировщик (qr.ts), с его таблицами; читает то, что показывает лист: байтовый
// режим, уровень M, версии 1–10. Путь — как у обычных читалок: порог по соседним блокам (при малом контрасте — один на
// весь снимок, по Оцу), три поисковых узора, перспектива по ним и выравнивающему узору, Рида — Соломона, зеркальный
// снимок.

import { EC_PER_BLOCK, EXP, LOG, TOTAL, blockSizes, formatSpots, formatWord, functionMap, maskAt, mul, versionSpot, versionWord, zigzag } from './qr.ts';

/** A picture as a canvas gives it: RGBA bytes, row by row. */
interface Picture {
    data: Uint8ClampedArray;
    width: number;
    height: number;
}

/** A dark-and-light bit picture: `bits[y * width + x]` 1 dark; with the grey levels and each 8×8 block's threshold. */
interface Bits {
    bits: Uint8Array;
    width: number;
    height: number;
    lum: Uint8Array;
    thresholds: Float32Array;
}

/** A finder pattern's centre (continuous coordinates: pixel p spans [p, p + 1)), its module size, how often seen. */
interface Centre {
    x: number;
    y: number;
    size: number;
    count: number;
}

/** 1 dark, 0 light, 2 outside the picture. */
const at = (b: Bits, x: number, y: number): number => (x >= 0 && y >= 0 && x < b.width && y < b.height ? b.bits[y * b.width + x] : 2);

const dist = (a: { x: number; y: number }, b: { x: number; y: number }): number => Math.hypot(a.x - b.x, a.y - b.y);

/** Grey levels; a see-through pixel counts as white paper. */
function luminance({ data, width, height }: Picture): Uint8Array {
    const lum = new Uint8Array(width * height);
    for (let i = 0; i < lum.length; i++) {
        const y = (data[4 * i] * 77 + data[4 * i + 1] * 150 + data[4 * i + 2] * 29) >> 8;
        const a = data[4 * i + 3];
        lum[i] = (y * a + 255 * (255 - a)) / 255;
    }
    return lum;
}

/** A box blur of the given radius, rows then columns: against noise and a screen's pixel grid. */
function blurred(lum: Uint8Array, width: number, height: number, radius: number): Uint8Array {
    const pass = (from: Uint8Array, along: number, across: number, step: number, line: number): Uint8Array => {
        const out = new Uint8Array(from.length);
        for (let l = 0; l < across; l++) {
            let sum = 0;
            const value = (i: number): number => from[l * line + Math.min(Math.max(i, 0), along - 1) * step];
            for (let i = -radius; i <= radius; i++) sum += value(i);
            for (let i = 0; i < along; i++) {
                out[l * line + i * step] = sum / (2 * radius + 1);
                sum += value(i + radius + 1) - value(i - radius);
            }
        }
        return out;
    };
    return pass(pass(lum, width, height, 1, width), height, width, width, 1);
}

/**
 * Dark and light by local means (as common readers do): an 8×8 block's black point is its mean, or — when the block is
 * flat — half its minimum, or its neighbours' when they are darker; a pixel is dark at or below the mean of the
 * black points of the 5×5 blocks round its own.
 */
function binarize(lum: Uint8Array, width: number, height: number): Bits {
    const columns = Math.ceil(width / 8);
    const rows = Math.ceil(height / 8);
    const points = new Uint8Array(columns * rows);
    const span = (block: number, limit: number): number => Math.max(0, Math.min(block * 8, limit - 8));
    for (let by = 0; by < rows; by++)
        for (let bx = 0; bx < columns; bx++) {
            let sum = 0;
            let min = 255;
            let max = 0;
            let count = 0;
            for (let y = span(by, height); y < Math.min(span(by, height) + 8, height); y++)
                for (let x = span(bx, width); x < Math.min(span(bx, width) + 8, width); x++) {
                    const v = lum[y * width + x];
                    sum += v;
                    count++;
                    if (v < min) min = v;
                    if (v > max) max = v;
                }
            let point = sum / count;
            if (max - min <= 24) {
                point = min / 2;
                if (bx > 0 && by > 0) {
                    const near = (points[(by - 1) * columns + bx] + 2 * points[by * columns + bx - 1] + points[(by - 1) * columns + bx - 1]) / 4;
                    if (min < near) point = near;
                }
            }
            points[by * columns + bx] = point;
        }
    const bits = new Uint8Array(width * height);
    const thresholds = new Float32Array(columns * rows);
    for (let by = 0; by < rows; by++)
        for (let bx = 0; bx < columns; bx++) {
            let sum = 0;
            let count = 0;
            for (let y = Math.max(0, by - 2); y <= Math.min(rows - 1, by + 2); y++)
                for (let x = Math.max(0, bx - 2); x <= Math.min(columns - 1, bx + 2); x++) {
                    sum += points[y * columns + x];
                    count++;
                }
            const threshold = (thresholds[by * columns + bx] = sum / count);
            for (let y = by * 8; y < Math.min(by * 8 + 8, height); y++)
                for (let x = bx * 8; x < Math.min(bx * 8 + 8, width); x++) bits[y * width + x] = lum[y * width + x] <= threshold ? 1 : 0;
        }
    return { bits, width, height, lum, thresholds };
}

/** Otsu's threshold over the grey levels in [from, to): the split that sets the two sides furthest apart. */
function otsu(counts: Float64Array, from: number, to: number): number {
    let [n, total] = [0, 0];
    for (let v = from; v < to; v++) [n, total] = [n + counts[v], total + v * counts[v]];
    let [below, belowSum, best, threshold] = [0, 0, -1, from];
    for (let t = from; t < to - 1; t++) {
        below += counts[t];
        belowSum += t * counts[t];
        const above = n - below;
        if (!below || !above) continue;
        const apart = below * above * (belowSum / below - (total - belowSum) / above) ** 2;
        if (apart > best) [best, threshold] = [apart, t];
    }
    return threshold;
}

/** Dark and light by one threshold for the whole picture. */
const cut = (lum: Uint8Array, width: number, height: number, threshold: number): Bits => ({
    bits: lum.map((v) => (v <= threshold ? 1 : 0)),
    width,
    height,
    lum,
    thresholds: new Float32Array(Math.ceil(width / 8) * Math.ceil(height / 8)).fill(threshold),
});

const sum = (list: number[]): number => list.reduce((a, b) => a + b, 0);

/** Five runs in the ratio 1:1:3:1:1, each within `slack` of a module. */
function finderRatio(c: number[], slack = 0.5): boolean {
    const total = sum(c);
    if (total < 7 || c.some((n) => n === 0)) return false;
    const module = total / 7;
    return c.every((n, i) => Math.abs((i === 2 ? 3 : 1) * module - n) < (i === 2 ? 3 : 1) * module * slack);
}

/**
 * Runs through (x, y) along (dx, dy) round a dark middle, `rings` more on each side (light, dark, …), each at most
 * `limit` long; with the middle's centre as an offset along the line. null — not that shape.
 */
function crossRuns(b: Bits, x: number, y: number, dx: number, dy: number, rings: number, limit: number): { counts: number[]; centre: number } | null {
    const run = (from: number, step: number, colour: number): number => {
        let n = 0;
        for (let i = from; n <= limit && at(b, x + dx * i, y + dy * i) === colour; i += step) n++;
        return n;
    };
    const back = run(0, -1, 1);
    const ahead = run(1, 1, 1);
    const before: number[] = [];
    const after: number[] = [];
    let i = -back;
    let j = 1 + ahead;
    for (let ring = 0; ring < rings; ring++) {
        const colour = ring % 2;
        const left = run(i, -1, colour);
        const right = run(j, 1, colour);
        before.unshift(left);
        after.push(right);
        i -= left;
        j += right;
    }
    const counts = [...before, back + ahead, ...after];
    return counts.some((n) => n === 0 || n > limit) ? null : { counts, centre: (ahead - back + 1) / 2 };
}

/** Finder patterns: rows scanned for 1:1:3:1:1, each hit confirmed down, across again and diagonally. */
function findCentres(b: Bits): Centre[] {
    const centres: Centre[] = [];
    const confirm = (counts: number[], row: number, end: number): void => {
        const total = sum(counts);
        const limit = counts[2] * 2;
        const column = Math.floor(end - counts[4] - counts[3] - counts[2] / 2);
        const down = crossRuns(b, column, row, 0, 1, 2, limit);
        if (!down || !finderRatio(down.counts) || Math.abs(sum(down.counts) - total) * 5 >= 2 * total) return;
        const y = row + 0.5 + down.centre;
        const across = crossRuns(b, column, Math.floor(y), 1, 0, 2, limit);
        if (!across || !finderRatio(across.counts) || Math.abs(sum(across.counts) - total) * 5 >= 2 * total) return;
        const x = column + 0.5 + across.centre;
        // One diagonal is enough: the other may run into a dark edge of the picture.
        const diagonal = (dy: number): boolean => {
            const runs = crossRuns(b, Math.floor(x), Math.floor(y), 1, dy, 2, limit * 2);
            return !!runs && finderRatio(runs.counts, 0.75);
        };
        if (!diagonal(1) && !diagonal(-1)) return;
        const size = (sum(across.counts) + sum(down.counts)) / 14;
        const near = centres.find((c) => Math.abs(c.x - x) <= c.size && Math.abs(c.y - y) <= c.size && Math.abs(c.size - size) <= Math.max(1, c.size));
        if (!near) centres.push({ x, y, size, count: 1 });
        else {
            const n = near.count;
            near.x = (near.x * n + x) / (n + 1);
            near.y = (near.y * n + y) / (n + 1);
            near.size = (near.size * n + size) / (n + 1);
            near.count = n + 1;
        }
    };
    const counts = [0, 0, 0, 0, 0];
    for (let y = 0; y < b.height; y++) {
        counts.fill(0);
        let state = 0;
        for (let x = 0; x <= b.width; x++) {
            const dark = x < b.width && b.bits[y * b.width + x] === 1;
            if (dark) {
                if (state % 2 === 1) state++;
                counts[state]++;
            } else if (state % 2 === 1) counts[state]++;
            else if (state < 4) counts[++state]++;
            else {
                if (finderRatio(counts)) confirm(counts.slice(), y, x);
                counts.copyWithin(0, 2);
                counts[3] = 1;
                counts[4] = 0;
                state = 3;
            }
        }
    }
    const seen = centres.filter((c) => c.count >= 2);
    return (seen.length >= 3 ? seen : centres).sort((a, c) => c.count - a.count).slice(0, 12);
}

/**
 * Triples of centres of about one module size, best first by how close they come to an isosceles right triangle;
 * each ordered bottom-left, top-left (at the right angle), top-right by the cross product.
 */
function triples(centres: Centre[]): Array<[Centre, Centre, Centre]> {
    const found: Array<{ score: number; three: [Centre, Centre, Centre] }> = [];
    for (let i = 0; i < centres.length; i++)
        for (let j = i + 1; j < centres.length; j++)
            for (let k = j + 1; k < centres.length; k++) {
                const three = [centres[i], centres[j], centres[k]];
                const sizes = three.map((c) => c.size);
                if (Math.max(...sizes) > 1.5 * Math.min(...sizes)) continue;
                // The corner at the right angle is opposite the longest side.
                const sides = [dist(three[1], three[2]), dist(three[0], three[2]), dist(three[0], three[1])];
                const corner = sides.indexOf(Math.max(...sides));
                const top = three[corner];
                let [a, c] = three.filter((_, n) => n !== corner);
                const [la, lc] = [dist(top, a), dist(top, c)];
                const hyp = sides[corner];
                if (hyp < 7 * Math.min(...sizes)) continue;
                const score = (Math.abs(hyp * hyp - 2 * la * la) + Math.abs(hyp * hyp - 2 * lc * lc)) / (hyp * hyp);
                if (score > 0.6) continue;
                if ((c.x - top.x) * (a.y - top.y) - (c.y - top.y) * (a.x - top.x) < 0) [a, c] = [c, a];
                found.push({ score, three: [a, top, c] });
            }
    return found.sort((p, q) => p.score - q.score).slice(0, 4).map((entry) => entry.three);
}

/**
 * Pixels from a finder's centre along the line to another point until dark, light, dark have passed (Bresenham):
 * half the finder's width along that line. NaN — no such runs.
 */
function darkLightDark(b: Bits, fromX: number, fromY: number, toX: number, toY: number): number {
    const steep = Math.abs(toY - fromY) > Math.abs(toX - fromX);
    if (steep) [fromX, fromY, toX, toY] = [fromY, fromX, toY, toX];
    const dx = Math.abs(toX - fromX);
    const dy = Math.abs(toY - fromY);
    const xStep = fromX < toX ? 1 : -1;
    const yStep = fromY < toY ? 1 : -1;
    let error = -Math.floor(dx / 2);
    let state = 0;
    for (let x = fromX, y = fromY; x !== toX + xStep; x += xStep) {
        const dark = (steep ? at(b, y, x) : at(b, x, y)) === 1;
        if ((state === 1) === dark) {
            if (state === 2) return Math.hypot(x - fromX, y - fromY);
            state++;
        }
        error += dy;
        if (error > 0) {
            if (y === toY) break;
            y += yStep;
            error -= dx;
        }
    }
    return state === 2 ? Math.hypot(toX + xStep - fromX, toY - fromY) : NaN;
}

/** The finder's whole width along the line through both centres, both ways, the picture's edge respected. */
function widthAlong(b: Bits, from: Centre, to: Centre): number {
    const [fx, fy, tx, ty] = [Math.floor(from.x), Math.floor(from.y), Math.floor(to.x), Math.floor(to.y)];
    let result = darkLightDark(b, fx, fy, tx, ty);
    let scale = 1;
    let ox = fx - (tx - fx);
    if (ox < 0) {
        scale = fx / (fx - ox);
        ox = 0;
    } else if (ox >= b.width) {
        scale = (b.width - 1 - fx) / (ox - fx);
        ox = b.width - 1;
    }
    let oy = Math.floor(fy - (ty - fy) * scale);
    scale = 1;
    if (oy < 0) {
        scale = fy / (fy - oy);
        oy = 0;
    } else if (oy >= b.height) {
        scale = (b.height - 1 - fy) / (oy - fy);
        oy = b.height - 1;
    }
    ox = Math.floor(fx + (ox - fx) * scale);
    result += darkLightDark(b, fx, fy, ox, oy);
    return result - 1;
}

/** Module size between two finders, measured on both of them. */
function moduleBetween(b: Bits, p: Centre, q: Centre): number {
    const one = widthAlong(b, p, q);
    const two = widthAlong(b, q, p);
    if (Number.isNaN(one)) return two / 7;
    if (Number.isNaN(two)) return one / 7;
    return (one + two) / 14;
}

/**
 * The alignment pattern near where it should be, found in the code's own directions (so a turned picture is no
 * harder): its 5×5 rings matched cell by cell at every offset round the guess from the finders, coarse then pixel by
 * pixel, and the centre of the best matches taken. null — nothing matches well enough.
 */
function findAlignment(b: Bits, tl: Centre, tr: Centre, bl: Centre, size: number): { x: number; y: number } | null {
    const span = size - 7;
    const [ux, uy, vx, vy] = [(tr.x - tl.x) / span, (tr.y - tl.y) / span, (bl.x - tl.x) / span, (bl.y - tl.y) / span];
    const module = (Math.hypot(ux, uy) + Math.hypot(vx, vy)) / 2;
    const ex = tl.x + (span - 3) * (ux + vx);
    const ey = tl.y + (span - 3) * (uy + vy);
    const score = (cx: number, cy: number): number => {
        let s = 0;
        for (let j = -2; j <= 2; j++)
            for (let i = -2; i <= 2; i++)
                if (at(b, Math.floor(cx + i * ux + j * vx), Math.floor(cy + i * uy + j * vy)) === (Math.max(Math.abs(i), Math.abs(j)) === 1 ? 0 : 1)) s++;
        return s;
    };
    for (const allowance of [4, 8]) {
        const reach = allowance * module;
        const step = Math.max(1, module / 2);
        let best = { x: ex, y: ey, s: -1 };
        for (let oy = -reach; oy <= reach; oy += step)
            for (let ox = -reach; ox <= reach; ox += step) {
                const s = score(ex + ox, ey + oy);
                if (s > best.s || (s === best.s && Math.hypot(ox, oy) < Math.hypot(best.x - ex, best.y - ey))) best = { x: ex + ox, y: ey + oy, s };
            }
        let top = -1;
        let xs = 0;
        let ys = 0;
        let n = 0;
        for (let oy = -module; oy <= module; oy++)
            for (let ox = -module; ox <= module; ox++) {
                const s = score(best.x + ox, best.y + oy);
                if (s > top) [top, xs, ys, n] = [s, 0, 0, 0];
                if (s === top) [xs, ys, n] = [xs + best.x + ox, ys + best.y + oy, n + 1];
            }
        if (top >= 22) return { x: xs / n, y: ys / n };
    }
    return null;
}

type Matrix = number[];

/** The projective map of the unit square onto a quadrilateral (corners clockwise from the top-left). */
function squareTo(x0: number, y0: number, x1: number, y1: number, x2: number, y2: number, x3: number, y3: number): Matrix {
    const dx3 = x0 - x1 + x2 - x3;
    const dy3 = y0 - y1 + y2 - y3;
    if (dx3 === 0 && dy3 === 0) return [x1 - x0, x2 - x1, x0, y1 - y0, y2 - y1, y0, 0, 0, 1];
    const [dx1, dx2, dy1, dy2] = [x1 - x2, x3 - x2, y1 - y2, y3 - y2];
    const den = dx1 * dy2 - dx2 * dy1;
    const a13 = (dx3 * dy2 - dx2 * dy3) / den;
    const a23 = (dx1 * dy3 - dx3 * dy1) / den;
    return [x1 - x0 + a13 * x1, x3 - x0 + a23 * x3, x0, y1 - y0 + a13 * y1, y3 - y0 + a23 * y3, y0, a13, a23, 1];
}

/**
 * `a` after `b`. A map is [a11, a21, a31, a12, a22, a32, a13, a23, a33]: X = (a11·x + a21·y + a31) / w,
 * Y = (a12·x + a22·y + a32) / w, w = a13·x + a23·y + a33 — the 3×3 matrix row by row.
 */
function times(a: Matrix, b: Matrix): Matrix {
    const out: Matrix = [];
    for (let r = 0; r < 3; r++) for (let c = 0; c < 3; c++) out.push(a[3 * r] * b[c] + a[3 * r + 1] * b[3 + c] + a[3 * r + 2] * b[6 + c]);
    return out;
}

/** The adjoint: the inverse up to a factor, which a projective map does not mind. */
function adjoint([a11, a21, a31, a12, a22, a32, a13, a23, a33]: Matrix): Matrix {
    return [
        a22 * a33 - a23 * a32, a23 * a31 - a21 * a33, a21 * a32 - a22 * a31,
        a13 * a32 - a12 * a33, a11 * a33 - a13 * a31, a12 * a31 - a11 * a32,
        a12 * a23 - a13 * a22, a13 * a21 - a11 * a23, a11 * a22 - a12 * a21,
    ];
}

/** Every module read at its centre through the perspective of the finders and the alignment pattern (or a guess). */
function sample(b: Bits, tl: Centre, tr: Centre, bl: Centre, align: { x: number; y: number } | null, size: number, module: number): Uint8Array | null {
    const far = size - 3.5;
    const [bx, by, source] = align ? [align.x, align.y, far - 3] : [tr.x - tl.x + bl.x, tr.y - tl.y + bl.y, far];
    const from = adjoint(squareTo(3.5, 3.5, far, 3.5, source, source, 3.5, far));
    const m = times(squareTo(tl.x, tl.y, tr.x, tr.y, bx, by, bl.x, bl.y), from);
    const grid = new Uint8Array(size * size);
    // The grey levels round the centre (3×3 when a module is large enough) averaged, then held against the threshold
    // of the block the centre is in: averaging first keeps noise from flipping modules.
    const reach = module >= 3.5 ? 1 : 0;
    const columns = Math.ceil(b.width / 8);
    for (let y = 0; y < size; y++)
        for (let x = 0; x < size; x++) {
            const den = m[6] * (x + 0.5) + m[7] * (y + 0.5) + m[8];
            const px = Math.floor((m[0] * (x + 0.5) + m[1] * (y + 0.5) + m[2]) / den);
            const py = Math.floor((m[3] * (x + 0.5) + m[4] * (y + 0.5) + m[5]) / den);
            if (!(px >= -1 && py >= -1 && px <= b.width && py <= b.height)) return null;
            const [cx, cy] = [Math.min(Math.max(px, 0), b.width - 1), Math.min(Math.max(py, 0), b.height - 1)];
            let total = 0;
            let count = 0;
            for (let dy = -reach; dy <= reach; dy++)
                for (let dx = -reach; dx <= reach; dx++)
                    if (at(b, cx + dx, cy + dy) !== 2) {
                        total += b.lum[(cy + dy) * b.width + cx + dx];
                        count++;
                    }
            grid[y * size + x] = total / count <= b.thresholds[(cy >> 3) * columns + (cx >> 3)] ? 1 : 0;
        }
    return grid;
}

const bitCount = (n: number): number => {
    let c = 0;
    for (; n; n &= n - 1) c++;
    return c;
};

const div = (a: number, b: number): number => (a ? EXP[(LOG[a] + 255 - LOG[b]) % 255] : 0);

/** A polynomial (lowest power first) at x. */
function evaluate(poly: ArrayLike<number>, x: number): number {
    let r = 0;
    for (let k = poly.length - 1; k >= 0; k--) r = mul(r, x) ^ poly[k];
    return r;
}

/** Syndromes of a block (data then error correction): S_j = r(αʲ). */
function syndromes(block: Uint8Array, ec: number): Uint8Array {
    const s = new Uint8Array(ec);
    for (let j = 0; j < ec; j++) for (const byte of block) s[j] = mul(s[j], EXP[j]) ^ byte;
    return s;
}

/**
 * Reed–Solomon correction of one block in place: syndromes, Berlekamp–Massey, Chien search, Forney. At most ⌊ec/2⌋
 * errors; false — more than that, or nothing consistent.
 */
function correct(block: Uint8Array, ec: number): boolean {
    const s = syndromes(block, ec);
    if (s.every((v) => v === 0)) return true;
    let locator = [1];
    let previous = [1];
    let length = 0;
    let shift = 1;
    let last = 1;
    for (let r = 0; r < ec; r++) {
        let delta = s[r];
        for (let i = 1; i <= length; i++) delta ^= mul(locator[i] ?? 0, s[r - i]);
        if (!delta) {
            shift++;
            continue;
        }
        const next = locator.slice();
        const scale = div(delta, last);
        previous.forEach((c, i) => {
            while (next.length <= i + shift) next.push(0);
            next[i + shift] ^= mul(scale, c);
        });
        if (2 * length <= r) {
            previous = locator;
            length = r + 1 - length;
            last = delta;
            shift = 1;
        } else shift++;
        locator = next;
    }
    if (2 * length > ec) return false;
    const n = block.length;
    const omega = new Uint8Array(ec);
    for (let i = 0; i < ec; i++) for (let j = 0; j <= i && j < locator.length; j++) omega[i] ^= mul(s[i - j], locator[j]);
    let found = 0;
    for (let i = 0; i < n; i++) {
        const power = n - 1 - i;
        const inverse = EXP[(255 - power) % 255];
        if (evaluate(locator, inverse) !== 0) continue;
        let slope = 0;
        for (let k = 1; k < locator.length; k += 2) slope ^= mul(locator[k], EXP[(LOG[inverse] * (k - 1)) % 255]);
        if (!slope) return false;
        block[i] ^= mul(EXP[power], div(evaluate(omega, inverse), slope));
        found++;
    }
    return found === length && syndromes(block, ec).every((v) => v === 0);
}

const orders = new Map<number, Int32Array>();

/** A sampled grid → the text, or null: format, version, unmask, codewords, Reed–Solomon, the byte segment. */
function readGrid(grid: Uint8Array, size: number): string | null {
    const version = (size - 17) / 4;
    // Both copies of the format bits against all 32 words; the nearest within three bits wins, and it must be level M.
    let mask = -1;
    let best = 4;
    for (const copy of [0, 2]) {
        let word = 0;
        for (let i = 0; i < 15; i++) {
            const spot = formatSpots(size, i);
            word |= grid[spot[copy + 1] * size + spot[copy]] << i;
        }
        for (let level = 0; level < 4; level++)
            for (let m = 0; m < 8; m++) {
                const d = bitCount(word ^ formatWord(level, m));
                if (d < best) {
                    best = d;
                    mask = level === 0 ? m : -1;
                }
            }
    }
    if (mask < 0) return null;
    if (version >= 7) {
        const word = (mirror: boolean): number => {
            let w = 0;
            for (let i = 0; i < 18; i++) {
                const [x, y] = versionSpot(size, i);
                w |= (mirror ? grid[x * size + y] : grid[y * size + x]) << i;
            }
            return w;
        };
        if (bitCount(word(false) ^ versionWord(version)) > 3 && bitCount(word(true) ^ versionWord(version)) > 3) return null;
    }
    const total = TOTAL[version - 1];
    let order = orders.get(version);
    if (!order) orders.set(version, (order = zigzag(size, functionMap(version))));
    const raw = new Uint8Array(total);
    for (let i = 0; i < total * 8; i++) {
        const p = order[i];
        raw[i >> 3] |= (grid[p] ^ maskAt(mask, p % size, Math.floor(p / size))) << (7 - (i & 7));
    }
    const sizes = blockSizes(version);
    const ec = EC_PER_BLOCK[version - 1];
    const blocks = sizes.map((n) => new Uint8Array(n + ec));
    let k = 0;
    for (let i = 0; i < sizes[sizes.length - 1]; i++) blocks.forEach((block, n) => i < sizes[n] && (block[i] = raw[k++]));
    for (let i = 0; i < ec; i++) blocks.forEach((block, n) => (block[sizes[n] + i] = raw[k++]));
    const data: number[] = [];
    for (let n = 0; n < blocks.length; n++) {
        if (!correct(blocks[n], ec)) return null;
        data.push(...blocks[n].subarray(0, sizes[n]));
    }
    let bit = 0;
    const read = (count: number): number => {
        let v = 0;
        for (let i = 0; i < count; i++, bit++) v = (v << 1) | ((data[bit >> 3] >>> (7 - (bit & 7))) & 1);
        return v;
    };
    if (read(4) !== 4) return null;
    const length = read(version < 10 ? 8 : 16);
    if (bit + 8 * length > 8 * data.length) return null;
    const bytes = Uint8Array.from({ length }, () => read(8));
    try {
        return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    } catch {
        return null;
    }
}

/** A grid mirrored over its main diagonal: what a mirrored picture samples to. */
function transpose(grid: Uint8Array, size: number): Uint8Array {
    const out = new Uint8Array(grid.length);
    for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) out[x * size + y] = grid[y * size + x];
    return out;
}

/** The finders found → each likely triple, the two nearest sizes, with and without the alignment pattern. */
function locate(b: Bits): string | null {
    for (const [bl, tl, tr] of triples(findCentres(b))) {
        let module = (moduleBetween(b, tl, tr) + moduleBetween(b, tl, bl)) / 2;
        if (!(module >= 1)) module = (tl.size + tr.size + bl.size) / 3;
        const estimate = (dist(tl, tr) + dist(tl, bl)) / (2 * module) + 7;
        const sizes = [21, 25, 29, 33, 37, 41, 45, 49, 53, 57].sort((p, q) => Math.abs(p - estimate) - Math.abs(q - estimate)).slice(0, 2);
        for (const size of sizes) {
            const align = size > 21 ? findAlignment(b, tl, tr, bl, size) : null;
            for (const point of align ? [align, null] : [null]) {
                const grid = sample(b, tl, tr, bl, point, size, module);
                const text = grid && (readGrid(grid, size) ?? readGrid(transpose(grid, size), size));
                if (text != null) return text;
            }
        }
    }
    return null;
}

/** The text of the first QR code found in a picture (a photo of a screen or paper), or null. Byte mode, level M,
 *  versions 1–10 — what «Подключить шлюз» shows. */
export function decodeQr(image: Picture): string | null {
    const { width, height } = image;
    if (width < 21 || height < 21) return null;
    const lum = luminance(image);
    // As it is, then softened against noise and a screen's grid, then sharpened for small blurred modules.
    let soft: Uint8Array | null = null;
    const softened = (): Uint8Array => (soft ??= blurred(lum, width, height, 2));
    const passes = [
        () => lum,
        () => blurred(lum, width, height, 1),
        softened,
        () => {
            const s = softened();
            return lum.map((v, i) => Math.min(255, Math.max(0, v + 1.5 * (v - s[i]))));
        },
    ];
    for (const pass of passes) {
        const text = locate(binarize(pass(), width, height));
        if (text !== null) return text;
    }
    // Little contrast and large modules (a lamp's glare on a screen held close): the local means take the finders' flat
    // dark cores for light. One threshold for the whole softened picture instead — Otsu's split, then the split inside
    // each side of it, for a dark bezel or a bright wall round the code.
    const smooth = softened();
    const counts = new Float64Array(256);
    for (const v of smooth) counts[v]++;
    const whole = otsu(counts, 0, 256);
    for (const threshold of [whole, otsu(counts, whole + 1, 256), otsu(counts, 0, whole + 1)]) {
        const text = locate(cut(smooth, width, height, threshold));
        if (text !== null) return text;
    }
    return null;
}
