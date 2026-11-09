// Салют: конфетти на пару секунд, когда колонка подключилась (screens/connect.tsx). Холст поверх всего, клики сквозь
// него проходят; два залпа снизу по краям навстречу друг другу и один сверху по центру, потом бумажки падают,
// кружатся и гаснут. Цвета — ДиВи (лайм и тёмный) и тёплые акценты. При prefers-reduced-motion салюта нет.

const COLORS = ['#B6F36A', '#D7FA9E', '#0B0F0A', '#8B7CF6', '#FFB86B', '#FF7A90', '#6EC8FF'];
const LIFE = 2600;

interface Piece {
    x: number;
    y: number;
    vx: number;
    vy: number;
    spin: number;
    angle: number;
    w: number;
    h: number;
    color: string;
    wobble: number;
}

function reduced(): boolean {
    try {
        return matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
        return false;
    }
}

/** Один салют. Повторный вызов, пока идёт прежний, добавляет новый холст — каждый убирает себя сам. */
export function confetti(): void {
    if (reduced() || typeof document === 'undefined') return;
    const canvas = document.createElement('canvas');
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    const width = window.innerWidth;
    const height = window.innerHeight;
    canvas.width = width * ratio;
    canvas.height = height * ratio;
    canvas.setAttribute('aria-hidden', 'true');
    canvas.style.cssText = 'position:fixed;inset:0;width:100vw;height:100vh;pointer-events:none;z-index:1000';
    document.body.appendChild(canvas);
    const ctx = canvas.getContext('2d');
    if (!ctx) return canvas.remove();
    ctx.scale(ratio, ratio);

    const pieces: Piece[] = [];
    const scale = Math.min(1, width / 900) * 0.6 + 0.4;
    const salvo = (x: number, y: number, angle: number, spread: number, count: number, speed: number) => {
        for (let i = 0; i < count; i++) {
            const a = angle + (Math.random() - 0.5) * spread;
            const v = speed * (0.55 + Math.random() * 0.6) * scale;
            pieces.push({
                x,
                y,
                vx: Math.cos(a) * v,
                vy: Math.sin(a) * v,
                spin: (Math.random() - 0.5) * 0.4,
                angle: Math.random() * Math.PI,
                w: 6 + Math.random() * 6,
                h: 4 + Math.random() * 5,
                color: COLORS[Math.floor(Math.random() * COLORS.length)],
                wobble: Math.random() * Math.PI * 2,
            });
        }
    };
    // Снизу слева — вверх-вправо, снизу справа — вверх-влево, сверху по центру — веером вниз.
    salvo(width * 0.08, height * 0.95, -Math.PI / 3, 0.7, 70, 19);
    salvo(width * 0.92, height * 0.95, (-2 * Math.PI) / 3, 0.7, 70, 19);
    salvo(width * 0.5, height * 0.3, -Math.PI / 2, Math.PI * 1.4, 60, 11);

    const start = performance.now();
    let last = start;
    const frame = (now: number) => {
        const age = now - start;
        const dt = Math.min(32, now - last) / 16.67;
        last = now;
        ctx.clearRect(0, 0, width, height);
        const fade = age > LIFE - 600 ? Math.max(0, (LIFE - age) / 600) : 1;
        ctx.globalAlpha = fade;
        for (const p of pieces) {
            p.vy += 0.32 * dt;
            p.vx *= Math.pow(0.985, dt);
            p.vy *= Math.pow(0.985, dt);
            p.wobble += 0.12 * dt;
            p.x += (p.vx + Math.sin(p.wobble) * 0.6) * dt;
            p.y += p.vy * dt;
            p.angle += p.spin * dt;
            ctx.save();
            ctx.translate(p.x, p.y);
            ctx.rotate(p.angle);
            // Бумажка переворачивается: ширина «дышит» по синусу.
            ctx.scale(Math.cos(p.wobble), 1);
            ctx.fillStyle = p.color;
            ctx.fillRect(-p.w / 2, -p.h / 2, p.w, p.h);
            ctx.restore();
        }
        if (age < LIFE) requestAnimationFrame(frame);
        else canvas.remove();
    };
    requestAnimationFrame(frame);
}
