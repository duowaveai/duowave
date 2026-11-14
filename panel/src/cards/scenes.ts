// Живые сцены за карточкой (`look.scene`, gateway/src/bus/cards.rs, SCENES): дождь, снег, солнце, звёзды, сияние,
// волны… Рисует их пульт на canvas 2D — приложение только называет сцену. Один цикл кадров на все сцены стола, 30
// кадров в секунду; сцена вне экрана, в скрытой вкладке или при «меньше движения» стоит (один кадр).

export type SceneName =
    | 'sun'
    | 'clouds'
    | 'rain'
    | 'storm'
    | 'snow'
    | 'fog'
    | 'stars'
    | 'aurora'
    | 'waves'
    | 'bubbles'
    | 'sparkles'
    | 'grid';

/** Цвета фона сцены, когда приложение своих не дало. */
export const SCENE_COLORS: Record<SceneName, string[]> = {
    sun: ['#1f6fd6', '#58aef5', '#9fd4ff'],
    clouds: ['#4f73b8', '#86a7d8'],
    rain: ['#2f3d52', '#55708e'],
    storm: ['#161b26', '#36414f'],
    snow: ['#5f8ccc', '#a9cbef'],
    fog: ['#6c7887', '#aab5c1'],
    stars: ['#070b2a', '#211c57', '#3a2a73'],
    aurora: ['#04112a', '#0b2f45'],
    waves: ['#5b2a91', '#9b3fd6'],
    bubbles: ['#0f8a80', '#2fd17a'],
    sparkles: ['#e2721a', '#f6b81d'],
    grid: ['#12072e', '#3b0a5c', '#6d1677'],
};

export const SCENES = Object.keys(SCENE_COLORS) as SceneName[];

type Rng = () => number;

/** Повторяемый случай: у одной сцены одни и те же облака после перерисовки. */
function seeded(seed: number): Rng {
    let a = seed >>> 0;
    return () => {
        a = (a + 0x6d2b79f5) >>> 0;
        let t = a;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

interface Particle {
    x: number;
    y: number;
    r: number;
    v: number;
    p: number;
    a: number;
}

export interface SceneState {
    name: SceneName;
    w: number;
    h: number;
    items: Particle[];
    clouds: Particle[];
    flash: number;
    nextFlash: number;
    shooting: { x: number; y: number; vx: number; vy: number; life: number } | null;
    nextShooting: number;
    rng: Rng;
}

const count = (w: number, h: number, per: number, min: number, max: number): number =>
    Math.round(Math.max(min, Math.min(max, (w * h) / per)));

function particles(rng: Rng, n: number, w: number, h: number, make: (rng: Rng) => Partial<Particle>): Particle[] {
    return Array.from({ length: n }, () => ({ x: rng() * w, y: rng() * h, r: 1, v: 1, p: rng() * Math.PI * 2, a: 1, ...make(rng) }));
}

export function sceneState(name: SceneName, w: number, h: number, seed = 7): SceneState {
    const rng = seeded(seed);
    const state: SceneState = { name, w, h, items: [], clouds: [], flash: 0, nextFlash: 2 + rng() * 3, shooting: null, nextShooting: 3 + rng() * 4, rng };
    const cloudy = (n: number, alpha: number): Particle[] =>
        particles(rng, n, w, h * 0.55, (r) => ({ r: h * (0.16 + r() * 0.18), v: 4 + r() * 9, a: alpha * (0.6 + r() * 0.4) }));
    switch (name) {
        case 'sun':
            state.items = particles(rng, count(w, h, 2600, 10, 26), w, h, (r) => ({ r: 1 + r() * 2.4, v: 6 + r() * 12, a: 0.25 + r() * 0.4 }));
            break;
        case 'clouds':
            state.clouds = cloudy(6, 0.42);
            break;
        case 'rain':
            state.clouds = cloudy(4, 0.22);
            state.items = particles(rng, count(w, h, 900, 30, 110), w, h, (r) => ({ r: 9 + r() * 10, v: 380 + r() * 180, a: 0.22 + r() * 0.3 }));
            break;
        case 'storm':
            state.clouds = cloudy(5, 0.2);
            state.items = particles(rng, count(w, h, 700, 40, 140), w, h, (r) => ({ r: 12 + r() * 12, v: 520 + r() * 220, a: 0.2 + r() * 0.3 }));
            break;
        case 'snow':
            state.items = particles(rng, count(w, h, 1200, 24, 90), w, h, (r) => ({ r: 0.9 + r() * 2.6, v: 14 + r() * 26, a: 0.55 + r() * 0.4 }));
            break;
        case 'fog':
            state.items = particles(rng, 5, w, h, (r) => ({ y: h * (0.25 + r() * 0.7), r: h * (0.18 + r() * 0.16), v: 6 + r() * 10, a: 0.14 + r() * 0.12 }));
            break;
        case 'stars':
            state.items = particles(rng, count(w, h, 900, 30, 120), w, h * 0.92, (r) => ({ r: r() < 0.12 ? 1.6 : 0.6 + r() * 0.8, v: 0.6 + r() * 1.8, a: 0.4 + r() * 0.6 }));
            break;
        case 'aurora':
            state.items = particles(rng, count(w, h, 1800, 14, 50), w, h * 0.8, (r) => ({ r: 0.5 + r() * 1, v: 0.6 + r() * 1.6, a: 0.3 + r() * 0.5 }));
            break;
        case 'bubbles':
            state.items = particles(rng, count(w, h, 2400, 12, 40), w, h, (r) => ({ r: 2.5 + r() * 8, v: 10 + r() * 22, a: 0.25 + r() * 0.35 }));
            break;
        case 'sparkles':
            state.items = particles(rng, count(w, h, 2600, 10, 30), w, h, (r) => ({ r: 2 + r() * 4.5, v: 0.8 + r() * 1.8, a: 0.5 + r() * 0.5 }));
            break;
        default:
            break;
    }
    return state;
}

function cloud(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, alpha: number, tint = '255, 255, 255'): void {
    const puffs: Array<[number, number, number]> = [
        [-0.55, 0.1, 0.5],
        [-0.15, -0.18, 0.62],
        [0.3, -0.05, 0.55],
        [0.65, 0.12, 0.42],
        [0.05, 0.2, 0.6],
    ];
    for (const [dx, dy, k] of puffs) {
        const cx = x + dx * r;
        const cy = y + dy * r;
        const radius = r * k;
        const g = ctx.createRadialGradient(cx, cy, radius * 0.1, cx, cy, radius);
        g.addColorStop(0, `rgba(${tint}, ${alpha})`);
        g.addColorStop(1, `rgba(${tint}, 0)`);
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(cx, cy, radius, 0, Math.PI * 2);
        ctx.fill();
    }
}

function drift(items: Particle[], w: number, dt: number, pad: number): void {
    for (const item of items) {
        item.x += item.v * dt;
        if (item.x - pad > w) item.x = -pad;
    }
}

function drawClouds(ctx: CanvasRenderingContext2D, s: SceneState, dt: number, tint?: string): void {
    drift(s.clouds, s.w, dt, s.h * 0.4);
    for (const c of s.clouds) cloud(ctx, c.x, c.y, c.r, c.a, tint);
}

function drawRain(ctx: CanvasRenderingContext2D, s: SceneState, dt: number, slant: number): void {
    ctx.lineCap = 'round';
    for (const d of s.items) {
        d.y += d.v * dt;
        d.x += d.v * slant * dt;
        if (d.y - d.r > s.h) {
            d.y = -d.r - s.rng() * 30;
            d.x = s.rng() * (s.w + 40) - 20;
        }
        if (d.x > s.w + 20) d.x -= s.w + 40;
        ctx.strokeStyle = `rgba(220, 235, 255, ${d.a})`;
        ctx.lineWidth = 1.2;
        ctx.beginPath();
        ctx.moveTo(d.x, d.y);
        ctx.lineTo(d.x - d.r * slant, d.y - d.r);
        ctx.stroke();
    }
}

function sparkle(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, alpha: number): void {
    ctx.fillStyle = `rgba(255, 255, 240, ${alpha})`;
    ctx.beginPath();
    ctx.moveTo(x, y - r);
    ctx.quadraticCurveTo(x, y, x + r, y);
    ctx.quadraticCurveTo(x, y, x, y + r);
    ctx.quadraticCurveTo(x, y, x - r, y);
    ctx.quadraticCurveTo(x, y, x, y - r);
    ctx.fill();
}

/** Один кадр сцены. `t` — секунды от начала, `dt` — с прошлого кадра (0 — стоп-кадр). */
export function drawScene(ctx: CanvasRenderingContext2D, s: SceneState, t: number, dt: number): void {
    const { w, h } = s;
    ctx.clearRect(0, 0, w, h);
    switch (s.name) {
        case 'sun': {
            const sx = w * 0.84;
            const sy = h * 0.2;
            const R = Math.min(w, h) * 0.13;
            const glow = ctx.createRadialGradient(sx, sy, R * 0.5, sx, sy, Math.max(w, h) * 0.75);
            glow.addColorStop(0, 'rgba(255, 244, 205, 0.55)');
            glow.addColorStop(0.35, 'rgba(255, 226, 160, 0.16)');
            glow.addColorStop(1, 'rgba(255, 226, 160, 0)');
            ctx.fillStyle = glow;
            ctx.fillRect(0, 0, w, h);
            ctx.save();
            ctx.translate(sx, sy);
            ctx.rotate(t * 0.06);
            for (let k = 0; k < 12; k += 1) {
                ctx.rotate((Math.PI * 2) / 12);
                const long = Math.max(w, h) * (k % 2 ? 0.55 : 0.8);
                const ray = ctx.createLinearGradient(0, 0, long, 0);
                ray.addColorStop(0, 'rgba(255, 248, 220, 0.22)');
                ray.addColorStop(1, 'rgba(255, 248, 220, 0)');
                ctx.fillStyle = ray;
                ctx.beginPath();
                ctx.moveTo(R * 0.8, -R * 0.12);
                ctx.lineTo(long, -R * 0.55);
                ctx.lineTo(long, R * 0.55);
                ctx.lineTo(R * 0.8, R * 0.12);
                ctx.fill();
            }
            ctx.restore();
            const disc = ctx.createRadialGradient(sx - R * 0.25, sy - R * 0.25, R * 0.1, sx, sy, R);
            disc.addColorStop(0, '#fffdf2');
            disc.addColorStop(1, '#ffe08a');
            ctx.fillStyle = disc;
            ctx.beginPath();
            ctx.arc(sx, sy, R * (1 + Math.sin(t * 1.4) * 0.025), 0, Math.PI * 2);
            ctx.fill();
            for (const m of s.items) {
                m.y -= m.v * dt;
                if (m.y < -4) {
                    m.y = h + 4;
                    m.x = s.rng() * w;
                }
                ctx.fillStyle = `rgba(255, 250, 225, ${m.a * (0.6 + 0.4 * Math.sin(t * 2 + m.p))})`;
                ctx.beginPath();
                ctx.arc(m.x + Math.sin(t * 0.8 + m.p) * 6, m.y, m.r, 0, Math.PI * 2);
                ctx.fill();
            }
            break;
        }
        case 'clouds':
            drawClouds(ctx, s, dt);
            break;
        case 'rain':
            drawClouds(ctx, s, dt, '230, 238, 248');
            drawRain(ctx, s, dt, 0.18);
            break;
        case 'storm': {
            drawClouds(ctx, s, dt, '200, 210, 225');
            drawRain(ctx, s, dt, 0.28);
            s.nextFlash -= dt;
            if (s.nextFlash <= 0) {
                s.flash = 1;
                s.nextFlash = 3 + s.rng() * 5;
            }
            if (s.flash > 0) {
                ctx.fillStyle = `rgba(235, 240, 255, ${s.flash * 0.45})`;
                ctx.fillRect(0, 0, w, h);
                if (s.flash > 0.6) {
                    ctx.strokeStyle = `rgba(255, 255, 255, ${s.flash})`;
                    ctx.lineWidth = 2;
                    ctx.beginPath();
                    let x = w * (0.25 + s.rng() * 0.5);
                    let y = 0;
                    ctx.moveTo(x, y);
                    while (y < h * 0.7) {
                        x += (s.rng() - 0.5) * 26;
                        y += 12 + s.rng() * 16;
                        ctx.lineTo(x, y);
                    }
                    ctx.stroke();
                }
                s.flash = Math.max(0, s.flash - dt * 3.2);
            }
            break;
        }
        case 'snow':
            for (const f of s.items) {
                f.y += f.v * dt;
                if (f.y - f.r > h) {
                    f.y = -f.r - s.rng() * 20;
                    f.x = s.rng() * w;
                }
                ctx.fillStyle = `rgba(255, 255, 255, ${f.a})`;
                ctx.beginPath();
                ctx.arc(f.x + Math.sin(t * 0.9 + f.p) * (4 + f.r * 2), f.y, f.r, 0, Math.PI * 2);
                ctx.fill();
            }
            break;
        case 'fog':
            for (const band of s.items) {
                const x = ((band.x + t * band.v) % (w * 1.6)) - w * 0.3;
                const g = ctx.createRadialGradient(x, band.y, band.r * 0.2, x, band.y, band.r * 3);
                g.addColorStop(0, `rgba(255, 255, 255, ${band.a})`);
                g.addColorStop(1, 'rgba(255, 255, 255, 0)');
                ctx.fillStyle = g;
                ctx.save();
                ctx.translate(x, band.y);
                ctx.scale(3, 1);
                ctx.translate(-x, -band.y);
                ctx.beginPath();
                ctx.arc(x, band.y, band.r * 3, 0, Math.PI * 2);
                ctx.fill();
                ctx.restore();
            }
            break;
        case 'stars': {
            for (const star of s.items) {
                const a = star.a * (0.45 + 0.55 * (0.5 + 0.5 * Math.sin(t * star.v + star.p)));
                ctx.fillStyle = `rgba(235, 240, 255, ${a})`;
                ctx.beginPath();
                ctx.arc(star.x, star.y, star.r, 0, Math.PI * 2);
                ctx.fill();
            }
            const mx = w * 0.82;
            const my = h * 0.24;
            const mr = Math.min(w, h) * 0.1;
            const moon = ctx.createRadialGradient(mx, my, mr * 0.5, mx, my, mr * 3);
            moon.addColorStop(0, 'rgba(240, 236, 255, 0.25)');
            moon.addColorStop(1, 'rgba(240, 236, 255, 0)');
            ctx.fillStyle = moon;
            ctx.fillRect(mx - mr * 3, my - mr * 3, mr * 6, mr * 6);
            ctx.fillStyle = '#f6f1ff';
            ctx.beginPath();
            ctx.arc(mx, my, mr, 0, Math.PI * 2);
            ctx.fill();
            ctx.globalCompositeOperation = 'destination-out';
            ctx.beginPath();
            ctx.arc(mx + mr * 0.45, my - mr * 0.25, mr * 0.88, 0, Math.PI * 2);
            ctx.fill();
            ctx.globalCompositeOperation = 'source-over';
            s.nextShooting -= dt;
            if (!s.shooting && s.nextShooting <= 0 && dt > 0) {
                s.shooting = { x: s.rng() * w * 0.6, y: s.rng() * h * 0.3, vx: 260 + s.rng() * 160, vy: 90 + s.rng() * 60, life: 1 };
                s.nextShooting = 4 + s.rng() * 6;
            }
            if (s.shooting) {
                const m = s.shooting;
                m.x += m.vx * dt;
                m.y += m.vy * dt;
                m.life -= dt * 1.4;
                const tail = ctx.createLinearGradient(m.x, m.y, m.x - m.vx * 0.22, m.y - m.vy * 0.22);
                tail.addColorStop(0, `rgba(255, 255, 255, ${Math.max(0, m.life)})`);
                tail.addColorStop(1, 'rgba(255, 255, 255, 0)');
                ctx.strokeStyle = tail;
                ctx.lineWidth = 1.6;
                ctx.beginPath();
                ctx.moveTo(m.x, m.y);
                ctx.lineTo(m.x - m.vx * 0.22, m.y - m.vy * 0.22);
                ctx.stroke();
                if (m.life <= 0) s.shooting = null;
            }
            break;
        }
        case 'aurora': {
            for (const star of s.items) {
                ctx.fillStyle = `rgba(220, 240, 255, ${star.a * (0.5 + 0.5 * Math.sin(t * star.v + star.p))})`;
                ctx.fillRect(star.x, star.y, star.r * 1.4, star.r * 1.4);
            }
            ctx.globalCompositeOperation = 'lighter';
            const ribbons: Array<[string, number, number, number]> = [
                ['80, 255, 170', 0.32, 0.55, 0.0],
                ['130, 90, 255', 0.28, 0.42, 1.7],
                ['60, 200, 255', 0.24, 0.66, 3.1],
            ];
            for (const [rgb, alpha, base, phase] of ribbons) {
                const top = (x: number): number =>
                    h * (base - 0.22) + Math.sin(x / w * 5 + t * 0.5 + phase) * h * 0.08 + Math.sin(x / w * 11 - t * 0.8 + phase) * h * 0.035;
                const g = ctx.createLinearGradient(0, h * (base - 0.32), 0, h * (base + 0.2));
                g.addColorStop(0, `rgba(${rgb}, 0)`);
                g.addColorStop(0.45, `rgba(${rgb}, ${alpha})`);
                g.addColorStop(1, `rgba(${rgb}, 0)`);
                ctx.fillStyle = g;
                ctx.beginPath();
                ctx.moveTo(0, h);
                for (let x = 0; x <= w; x += 8) ctx.lineTo(x, top(x));
                ctx.lineTo(w, h);
                ctx.closePath();
                ctx.fill();
            }
            ctx.globalCompositeOperation = 'source-over';
            break;
        }
        case 'waves': {
            const layers: Array<[number, number, number, number]> = [
                [0.62, 0.2, 0.9, 0.12],
                [0.7, 0.16, 1.3, 0.16],
                [0.8, 0.12, 1.8, 0.22],
            ];
            for (const [base, amp, speed, alpha] of layers) {
                ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;
                ctx.beginPath();
                ctx.moveTo(0, h);
                for (let x = 0; x <= w; x += 6) {
                    const y = h * base + Math.sin(x / w * Math.PI * 3 + t * speed) * h * amp * 0.5 + Math.sin(x / w * Math.PI * 7 - t * speed * 1.6) * h * amp * 0.18;
                    ctx.lineTo(x, y);
                }
                ctx.lineTo(w, h);
                ctx.closePath();
                ctx.fill();
            }
            break;
        }
        case 'bubbles':
            for (const b of s.items) {
                b.y -= b.v * dt;
                if (b.y + b.r < 0) {
                    b.y = h + b.r + s.rng() * 20;
                    b.x = s.rng() * w;
                }
                const x = b.x + Math.sin(t * 1.2 + b.p) * 5;
                ctx.strokeStyle = `rgba(255, 255, 255, ${b.a})`;
                ctx.lineWidth = 1.2;
                ctx.beginPath();
                ctx.arc(x, b.y, b.r, 0, Math.PI * 2);
                ctx.stroke();
                ctx.fillStyle = `rgba(255, 255, 255, ${b.a * 0.9})`;
                ctx.beginPath();
                ctx.arc(x - b.r * 0.35, b.y - b.r * 0.35, Math.max(0.8, b.r * 0.22), 0, Math.PI * 2);
                ctx.fill();
            }
            break;
        case 'sparkles':
            for (const p of s.items) {
                const life = 0.5 + 0.5 * Math.sin(t * p.v + p.p);
                if (life < 0.03 && dt > 0) {
                    p.x = s.rng() * w;
                    p.y = s.rng() * h;
                }
                sparkle(ctx, p.x, p.y - t * 2 * p.v, p.r * (0.4 + life), p.a * life);
            }
            break;
        case 'grid': {
            const horizon = h * 0.52;
            const sun = ctx.createLinearGradient(0, horizon - h * 0.34, 0, horizon);
            sun.addColorStop(0, '#ffd36e');
            sun.addColorStop(1, '#ff4fa3');
            const R = Math.min(w * 0.22, h * 0.32);
            ctx.save();
            ctx.beginPath();
            ctx.arc(w * 0.5, horizon, R, Math.PI, 0);
            ctx.clip();
            ctx.fillStyle = sun;
            ctx.fillRect(w * 0.5 - R, horizon - R, R * 2, R);
            ctx.fillStyle = 'rgba(30, 8, 60, 0.85)';
            for (let k = 0; k < 5; k += 1) {
                const y = horizon - R * (0.12 + k * 0.17);
                ctx.fillRect(w * 0.5 - R, y, R * 2, 1.5 + k * 0.9);
            }
            ctx.restore();
            ctx.strokeStyle = 'rgba(255, 90, 210, 0.55)';
            ctx.lineWidth = 1;
            ctx.beginPath();
            const rows = 10;
            const shift = (t * 0.6) % 1;
            for (let k = 0; k < rows; k += 1) {
                const z = (k + shift) / rows;
                const y = horizon + (h - horizon) * z * z;
                ctx.moveTo(0, y);
                ctx.lineTo(w, y);
            }
            for (let k = -10; k <= 10; k += 1) {
                ctx.moveTo(w * 0.5 + k * w * 0.02, horizon);
                ctx.lineTo(w * 0.5 + k * w * 0.16, h);
            }
            ctx.stroke();
            ctx.strokeStyle = 'rgba(110, 240, 255, 0.7)';
            ctx.beginPath();
            ctx.moveTo(0, horizon);
            ctx.lineTo(w, horizon);
            ctx.stroke();
            break;
        }
        default:
            break;
    }
}

// ——— Один цикл на все сцены ———

interface Running {
    canvas: HTMLCanvasElement;
    state: SceneState;
    visible: boolean;
    started: number;
    last: number;
    drawn: boolean;
}

const running = new Set<Running>();
let frame = 0;
const FRAME_MS = 1000 / 30;

const still = (): boolean => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

function paint(entry: Running, now: number): void {
    const ctx = entry.canvas.getContext('2d');
    if (!ctx) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const moving = !still();
    const dt = moving && entry.drawn ? Math.min(0.1, (now - entry.last) / 1000) : 0;
    entry.last = now;
    entry.drawn = true;
    drawScene(ctx, entry.state, moving ? (now - entry.started) / 1000 : 12, dt);
}

function tick(now: number): void {
    frame = 0;
    if (!document.hidden && !still()) {
        for (const entry of running) {
            if (entry.visible && now - entry.last >= FRAME_MS - 2) paint(entry, now);
        }
    }
    if (running.size > 0) frame = requestAnimationFrame(tick);
}

/** Сцена на холсте, пока он в документе; вернёт, как её остановить. */
export function runScene(canvas: HTMLCanvasElement, name: SceneName, seed: number): () => void {
    const entry: Running = { canvas, state: sceneState(name, 1, 1, seed), visible: true, started: performance.now(), last: 0, drawn: false };
    const size = (): void => {
        const rect = canvas.getBoundingClientRect();
        const w = Math.max(1, Math.round(rect.width));
        const h = Math.max(1, Math.round(rect.height));
        const dpr = Math.min(2, window.devicePixelRatio || 1);
        if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
            canvas.width = Math.round(w * dpr);
            canvas.height = Math.round(h * dpr);
        }
        if (entry.state.w !== w || entry.state.h !== h) entry.state = sceneState(name, w, h, seed);
        entry.drawn = false;
        paint(entry, performance.now());
    };
    const resize = typeof ResizeObserver === 'function' ? new ResizeObserver(size) : null;
    resize?.observe(canvas);
    const seen =
        typeof IntersectionObserver === 'function'
            ? new IntersectionObserver((records) => {
                  for (const record of records) entry.visible = record.isIntersecting;
              })
            : null;
    seen?.observe(canvas);
    size();
    running.add(entry);
    if (!frame) frame = requestAnimationFrame(tick);
    return () => {
        running.delete(entry);
        resize?.disconnect();
        seen?.disconnect();
        if (running.size === 0 && frame) {
            cancelAnimationFrame(frame);
            frame = 0;
        }
    };
}
