// Живой ДиВи — знак DuoWave в шапке пульта (знак обновлён 13.11.2026: бренд-кит, logo/divi-mark.js; DIVI.md). Перенос
// divi-mark.js сайта: те же позы, тайминги и кривые, плюс остановка, когда знак уходит со страницы.
//
// В покое оба глаза открыты (капсулы) и смотрят. Сценка «Привет», около 3,5 с: оглядывается (глаза в сторону на 7 за
// 260 мс, 260 мс там, обратно за 220 мс) → моргает (80 + 130 мс) → 160 мс → подмигивает (правый глаз в дужку за 300 мс
// с перелётом) → 520 мс → открывает глаз (260 мс) → 280 мс → улыбается обеими дужками и подпрыгивает (присел 140 мс,
// вверх на 16 за 190 мс, приземлился 300 мс с перелётом) → 800 мс → снова смотрит (340 мс с перелётом).
// При загрузке (`intro`): 600 мс смотрит, потом сценка. При наведении на логотип (и при фокусе в нём) — сценка; пока
// она идёт, наведение её не перезапускает. При prefers-reduced-motion знак стоит с открытыми глазами.
//
// Сам по себе (`idle`): `true` — если 10–16 с ничего не происходило, моргает, примерно каждый третий раз играет сценку.
// `'lively'` — ДиВи входа и мастера подключения: каждые 3–6 с оглядывается в одну сторону и в другую и моргает; иногда
// только моргает, изредка играет сценку. `cheer` — при появлении сразу радость с прыжком: колонка подключилась. Пока
// страница скрыта или знак не на экране, он стоит.

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
    /** Сдвиг вбок: ДиВи смотрит в сторону. */
    dx: number;
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

const open = (x: number): Eye => ({
    ox: x,
    x1: x,
    y1: CY - 14,
    ax: x,
    ay: CY - 14 / 3,
    bx: x,
    by: CY + 14 / 3,
    x2: x,
    y2: CY + 14,
    w: 24 * K,
    sy: 1,
    dx: 0,
});

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
        dx: 0,
    };
}

const body = (change: Partial<Body> = {}): Body => ({ sx: 1, sy: 1, ty: 0, ...change });
const POSES = {
    listen: (): Pose => ({ l: open(L), r: open(R), b: body() }),
    /** Смотрит в сторону: оба глаза сдвинуты на `dx`. */
    look: (dx = 7): Pose => {
        const pose = POSES.listen();
        pose.l.dx = pose.r.dx = dx;
        return pose;
    },
    wink: (): Pose => ({ l: open(L), r: arch(R), b: body() }),
    happy: (): Pose => ({ l: arch(L), r: arch(R), b: body() }),
};

/** Тело знака на холсте 256×256 — для рисунка ДиВи в центре орбиты (stations/divi.tsx). */
export const DIVI_BODY = BODY;

/** Глаз позы как линия: путь, толщина и сдвиг вбок. */
export interface DiviEye {
    d: string;
    width: number;
    dx: number;
}

/**
 * Глаза ДиВи в позе, без движения: `open` — смотрит (слушает), `look` — смотрит в сторону (думает), `happy` —
 * улыбается дужками (говорит). Те же кривые, что у живого знака.
 */
export function diviEyes(pose: 'open' | 'look' | 'happy'): [DiviEye, DiviEye] {
    const drawn = pose === 'happy' ? POSES.happy() : pose === 'look' ? POSES.look() : POSES.listen();
    const eye = (one: Eye): DiviEye => ({
        d: `M${one.x1} ${one.y1}C${one.ax} ${one.ay} ${one.bx} ${one.by} ${one.x2} ${one.y2}`,
        width: one.w,
        dx: one.dx,
    });
    return [eye(drawn.l), eye(drawn.r)];
}

const clone = (pose: Pose): Pose => ({ l: { ...pose.l }, r: { ...pose.r }, b: { ...pose.b } });

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
/** Ease-out-back с перелётом 1,70158 — у приземления, у сгиба глаза в дужку и у возврата взгляда. */
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
    /** При загрузке: 600 мс смотрит, потом сценка «Привет». */
    intro?: boolean;
    /** Где наведение запускает сценку: обычно логотип целиком (знак и надпись). Без него — сам знак. */
    hover?: HTMLElement;
    /** Сам по себе: `true` — изредка моргает, `'lively'` — часто оглядывается (см. вверху). */
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
    let pose = POSES.listen();
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
            g.setAttribute('transform', `translate(${eye.ox + eye.dx} ${CY}) scale(1 ${eye.sy}) translate(${-eye.ox} ${-CY})`);
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

    /** Сценка «Привет»: оглядывается → моргает → подмигивает → открывает глаз → улыбается с прыжком → смотрит. */
    const greet = async (): Promise<void> => {
        if (busy || !alive) return;
        busy = true;
        await to(POSES.look(), 260);
        await wait(260);
        await to(POSES.listen(), 220);
        await blink();
        await wait(160);
        await to(POSES.wink(), 300, back);
        await wait(520);
        await to(POSES.listen(), 260);
        await wait(280);
        await hop();
        await wait(800);
        await to(POSES.listen(), 340, back);
        busy = false;
    };

    /** Колонка подключилась: сразу радость с прыжком, потом снова смотрит. */
    const cheer = async (): Promise<void> => {
        if (busy || !alive) return;
        busy = true;
        await hop();
        await wait(800);
        await to(POSES.listen(), 340, back);
        busy = false;
    };

    let idleTimer = 0;
    let restartIdle: (() => void) | undefined;

    const hover = (): void => {
        if (busy || !alive) return;
        restartIdle?.();
        void greet();
    };

    /** Посмотреть в одну сторону, в другую и снова прямо. */
    const lookAround = async (): Promise<void> => {
        const side = Math.random() < 0.5 ? -7 : 7;
        await to(POSES.look(side), 240);
        await wait(380);
        await to(POSES.look(-side), 320);
        await wait(380);
        await to(POSES.listen(), 260, back);
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
            if (lively ? roll < 0.15 : roll < 0.34) await greet();
            else {
                busy = true;
                if (lively && roll < 0.7) {
                    await lookAround();
                    await wait(180);
                }
                await blink();
                busy = false;
            }
            next();
        };
        restartIdle = next;
        next();
    };

    set(POSES.listen());
    const target = options.hover ?? host;
    if (!reduced()) {
        if (options.cheer) void wait(250).then(cheer);
        else if (options.intro) void wait(600).then(greet);
        if (options.idle) idle(options.idle === 'lively');
        target.addEventListener('mouseenter', hover);
        target.addEventListener('focusin', hover);
    }

    return {
        destroy: () => {
            alive = false;
            tween++;
            window.clearTimeout(idleTimer);
            for (const id of timers) window.clearTimeout(id);
            timers.clear();
            target.removeEventListener('mouseenter', hover);
            target.removeEventListener('focusin', hover);
        },
    };
}
