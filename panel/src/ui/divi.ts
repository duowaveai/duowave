// Живой ДиВи — знак DuoWave в шапке пульта (описание знака, «Состояния и анимации»; DESIGN.md). Перенос
// divi-mark.js сайта: те же позы, тайминги и кривые, плюс остановка, когда шапка уходит со страницы.
//
// При загрузке: глаза открыты 700 мс → моргнул (80 + 130 мс) → 200 мс → правый глаз гнётся в дужку за 300 мс с
// перелётом. При наведении на логотип (и при фокусе в нём): моргнул → обрадовался — обе дужки, присел за 140 мс,
// подпрыгнул на 16 px за 190 мс, приземлился за 300 мс с перелётом → 700 мс радости → снова подмигивает. Пока
// анимация идёт, наведение её не перезапускает. При prefers-reduced-motion знак стоит и подмигивает.
//
// Сам по себе (`idle`, как в engine.ts сайта): `true` — после 10–16 с тишины моргнёт, примерно каждый третий раз
// обрадуется целиком. `'lively'` — ДиВи входа и мастера подключения: каждые 3–6 с открывает оба глаза, оглядывается в
// одну сторону и в другую (глаза сдвигаются на 8), моргает обоими и снова подмигивает; иногда только моргает, изредка
// радуется с прыжком. `cheer` — при появлении сразу радость с прыжком: колонка подключилась. Пока страница скрыта или
// знак не на экране, он стоит.

const NS = 'http://www.w3.org/2000/svg';
const BODY = 'M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z';

/** Глаза 1,4×: центры x 98 и 158, y 124; дужка шире на треть прибавки толщины. */
const K = 1.4;
const CY = 124;
const L = 98;
const R = 158;
const HS = 1 + 0.33 * (K - 1);

/** Глаз — одна кубическая кривая: прямая у открытого, дуга окружности у дужки, поэтому они плавно перетекают. */
interface Eye {
    ox: number;
    x1: number;
    y1: number;
    ax: number;
    ay: number;
    bx: number;
    by: number;
    x2: number;
    y2: number;
    w: number;
    sy: number;
}
interface Body {
    sx: number;
    sy: number;
    ty: number;
}
interface Pose {
    l: Eye;
    r: Eye;
    b: Body;
}

const open = (x: number): Eye => ({ ox: x, x1: x, y1: CY - 14, ax: x, ay: CY - 14 / 3, bx: x, by: CY + 14 / 3, x2: x, y2: CY + 14, w: 24 * K, sy: 1 });

function arch(x: number): Eye {
    const a = 15 * HS;
    const h = 9;
    const y = CY + 5;
    const r = (a * a + h * h) / (2 * h);
    const cy = y - h + r;
    const aL = Math.atan2(y - cy, -a);
    const aR = Math.atan2(y - cy, a);
    const t = (4 / 3) * Math.tan((aR - aL) / 4) * r;
    return {
        ox: x,
        x1: x - a,
        y1: y,
        ax: x - a - t * Math.sin(aL),
        ay: y + t * Math.cos(aL),
        bx: x + a + t * Math.sin(aR),
        by: y - t * Math.cos(aR),
        x2: x + a,
        y2: y,
        w: 14 * K,
        sy: 1,
    };
}

const body = (change: Partial<Body> = {}): Body => ({ sx: 1, sy: 1, ty: 0, ...change });
const POSES = {
    wink: (): Pose => ({ l: open(L), r: arch(R), b: body() }),
    listen: (): Pose => ({ l: open(L), r: open(R), b: body() }),
    happy: (): Pose => ({ l: arch(L), r: arch(R), b: body() }),
};

const clone = (pose: Pose): Pose => ({ l: { ...pose.l }, r: { ...pose.r }, b: { ...pose.b } });

/** Глаза сдвинуты вбок на `dx` (единицы холста): ДиВи смотрит в сторону. */
function shifted(pose: Pose, dx: number): Pose {
    const eye = (e: Eye): Eye => ({ ...e, ox: e.ox + dx, x1: e.x1 + dx, ax: e.ax + dx, bx: e.bx + dx, x2: e.x2 + dx });
    return { l: eye(pose.l), r: eye(pose.r), b: { ...pose.b } };
}

function mix<T extends object>(from: T, to: T, t: number): T {
    const out = { ...from } as Record<string, number>;
    for (const key of Object.keys(to)) {
        const a = (from as Record<string, number>)[key];
        const b = (to as Record<string, number>)[key];
        // Центр глаза не плывёт: он прыгает сразу в конечное положение, как в divi-mark.js.
        out[key] = key === 'ox' ? b : a + (b - a) * t;
    }
    return out as T;
}
const lerp = (from: Pose, to: Pose, t: number): Pose => ({ l: mix(from.l, to.l, t), r: mix(from.r, to.r, t), b: mix(from.b, to.b, t) });

const ease = (t: number): number => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);
/** Ease-out-back с перелётом 1,70158 — у приземления и у сгиба глаза в дужку. */
function back(t: number): number {
    const c1 = 1.70158;
    const c3 = c1 + 1;
    return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
}

function reduced(): boolean {
    try {
        return matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
        return false;
    }
}

export interface DiviOptions {
    /** При загрузке: глаза открыты, моргнул, подмигнул. */
    intro?: boolean;
    /** Где наведение запускает радость с прыжком: обычно логотип целиком (знак и надпись). Без него — сам знак. */
    hover?: HTMLElement;
    /** Сам по себе: `true` — изредка моргает, `'lively'` — часто открывает глаза и оглядывается (см. вверху). */
    idle?: boolean | 'lively';
    /** При появлении — радость с прыжком. */
    cheer?: boolean;
}

export interface DiviMark {
    /** Снять слушателей и остановить анимацию: знак ушёл со страницы. */
    destroy: () => void;
}

/** Нарисовать живого ДиВи в `host` (размер — у host, знак во всю его ширину). Цвета — лайм и тёмные глаза. */
export function mountDivi(host: HTMLElement, options: DiviOptions = {}): DiviMark {
    let pose = POSES.wink();
    let tween = 0;
    let busy = false;
    let alive = true;
    const timers = new Set<number>();

    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '0 0 256 256');
    svg.setAttribute('aria-hidden', 'true');
    svg.style.cssText = 'display:block;width:100%;height:100%;overflow:visible';
    const group = document.createElementNS(NS, 'g');
    const shape = document.createElementNS(NS, 'path');
    shape.setAttribute('d', BODY);
    shape.setAttribute('fill', '#B6F36A');
    group.appendChild(shape);
    const eyes = [0, 1].map(() => {
        const g = document.createElementNS(NS, 'g');
        const path = document.createElementNS(NS, 'path');
        path.setAttribute('fill', 'none');
        path.setAttribute('stroke', '#0B0F0A');
        path.setAttribute('stroke-linecap', 'round');
        g.appendChild(path);
        group.appendChild(g);
        return { g, path };
    });
    svg.appendChild(group);
    host.replaceChildren(svg);

    const set = (next: Pose): void => {
        pose = next;
        const { b } = next;
        // Тело приседает и прыгает от низа по центру (128, 216).
        group.setAttribute('transform', `translate(0 ${b.ty}) translate(128 216) scale(${b.sx} ${b.sy}) translate(-128 -216)`);
        [next.l, next.r].forEach((eye, index) => {
            const { g, path } = eyes[index];
            g.setAttribute('transform', `translate(${eye.ox} ${CY}) scale(1 ${eye.sy}) translate(${-eye.ox} ${-CY})`);
            path.setAttribute('d', `M${eye.x1} ${eye.y1}C${eye.ax} ${eye.ay} ${eye.bx} ${eye.by} ${eye.x2} ${eye.y2}`);
            path.setAttribute('stroke-width', String(eye.w));
        });
    };

    const wait = (ms: number): Promise<void> =>
        new Promise((resolve) => {
            const id = window.setTimeout(() => {
                timers.delete(id);
                resolve();
            }, ms);
            timers.add(id);
        });

    const to = (target: Pose, ms: number, curve: (t: number) => number = ease): Promise<void> => {
        const from = clone(pose);
        const id = ++tween;
        return new Promise((resolve) => {
            const start = performance.now();
            const step = (now: number): void => {
                if (!alive || id !== tween) return resolve();
                const k = Math.min(1, (now - start) / ms);
                set(lerp(from, target, curve(k)));
                if (k < 1) requestAnimationFrame(step);
                else resolve();
            };
            requestAnimationFrame(step);
        });
    };

    /** Моргнуть: глаза сжимаются до 8 % по высоте за 80 мс и раскрываются за 130 мс. */
    const blink = async (): Promise<void> => {
        const was = clone(pose);
        const shut = clone(pose);
        shut.l.sy = shut.r.sy = 0.08;
        await to(shut, 80);
        await to(was, 130);
    };

    /** Обрадоваться: обе дужки, присесть 140 мс, подпрыгнуть на 16 px за 190 мс, приземлиться 300 мс с перелётом. */
    const hop = async (): Promise<void> => {
        const squat = POSES.happy();
        const up = POSES.happy();
        squat.b = body({ sx: 1.07, sy: 0.9 });
        up.b = body({ sx: 0.97, sy: 1.05, ty: -16 });
        await to(squat, 140);
        await to(up, 190);
        await to(POSES.happy(), 300, back);
    };

    const intro = async (): Promise<void> => {
        busy = true;
        await wait(700);
        await blink();
        await wait(200);
        await to(POSES.wink(), 300, back);
        busy = false;
    };

    let idleTimer = 0;
    let restartIdle: (() => void) | undefined;

    const hover = async (): Promise<void> => {
        if (busy || !alive) return;
        restartIdle?.();
        busy = true;
        await blink();
        await hop();
        await wait(700);
        await to(POSES.wink(), 300, back);
        busy = false;
    };

    /** Посмотреть в одну сторону, в другую и снова прямо: глаза сдвигаются на 8, как в «Слушает». */
    const look = async (): Promise<void> => {
        const ahead = clone(pose);
        const side = Math.random() < 0.5 ? -8 : 8;
        await to(shifted(ahead, side), 220);
        await wait(380);
        await to(shifted(ahead, -side), 300);
        await wait(380);
        await to(ahead, 220);
    };

    /** Проснуться: открыть оба глаза, оглядеться, моргнуть обоими и снова подмигнуть. Около 3 с. */
    const awake = async (): Promise<void> => {
        await to(POSES.listen(), 260, back);
        await wait(260);
        await look();
        await wait(180);
        await blink();
        await wait(320);
        await to(POSES.wink(), 300, back);
    };

    /** Только моргнуть обоими: открыть глаза, моргнуть, подмигнуть. */
    const twinkle = async (): Promise<void> => {
        await to(POSES.listen(), 200, back);
        await wait(240);
        await blink();
        await wait(200);
        await to(POSES.wink(), 300, back);
    };

    /** Знак виден: скрытый вариант схемы или ушедшая за край шапка стоят. */
    const onScreen = (): boolean => {
        const r = host.getBoundingClientRect();
        return r.width > 0 && r.bottom > 0 && r.top < window.innerHeight;
    };

    const idle = (lively: boolean): void => {
        const next = (): void => {
            window.clearTimeout(idleTimer);
            const ms = lively ? 3000 + Math.random() * 3000 : 10000 + Math.random() * 6000;
            if (alive) idleTimer = window.setTimeout(() => void tick(), ms);
        };
        const tick = async (): Promise<void> => {
            if (busy || !alive || document.hidden || !onScreen()) return next();
            const roll = Math.random();
            if (lively ? roll < 0.15 : roll < 0.34) await hover();
            else {
                busy = true;
                if (lively && roll < 0.7) await awake();
                else if (lively) await twinkle();
                else await blink();
                busy = false;
            }
            next();
        };
        restartIdle = next;
        next();
    };

    const still = reduced();
    set(options.intro && !still ? POSES.listen() : POSES.wink());
    const target = options.hover ?? host;
    const onHover = (): void => void hover();
    if (!still) {
        if (options.cheer) void wait(250).then(hover);
        else if (options.intro) void intro();
        if (options.idle) idle(options.idle === 'lively');
        target.addEventListener('mouseenter', onHover);
        target.addEventListener('focusin', onHover);
    }

    return {
        destroy: () => {
            alive = false;
            tween++;
            window.clearTimeout(idleTimer);
            for (const id of timers) window.clearTimeout(id);
            timers.clear();
            target.removeEventListener('mouseenter', onHover);
            target.removeEventListener('focusin', onHover);
        },
    };
}
