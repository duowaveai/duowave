// Комета агента (docs/plans/voice-creation.md, «Комета»; макет docs/design/voice-creation): фиолетовая точка уходит с
// орбиты от круга «Мозг» и летит к плитке-черновику, оставляя короткий след; долетела — у плитки рисуется пунктирный
// контур. Одна авторская анимация на весь путь, поверх стола, не задевая его. Сцены нет на экране, плитки не видно или
// человек просит меньше движения (`prefers-reduced-motion`) — без полёта: контур просто появляется.

/** Полёт кометы, мс: как в макете. */
const FLIGHT_MS = 950;

/** Середина элемента на экране, если он виден в окне. */
function centerOf(element: Element | null): { x: number; y: number } | null {
    if (!element) return null;
    const rect = element.getBoundingClientRect();
    if (rect.width === 0 || rect.bottom < 0 || rect.top > innerHeight || rect.right < 0 || rect.left > innerWidth) return null;
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
}

/** Круг «Мозг» на сцене колонки (screens/dashboard.tsx, data-node). */
export function brainCircle(): Element | null {
    return document.querySelector('.desk-scene [data-node="brain"] .dash-circle');
}

/**
 * Летит от круга «Мозг» к левому верхнему углу `to` — оттуда контур и рисуется (обход прямоугольника SVG начинается
 * там). Обещание — когда точка долетела (сразу, если лететь не надо). Летит одна: новая комета снимает прежнюю.
 */
export function flyComet(to: Element): Promise<void> {
    const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const from = centerOf(brainCircle());
    const target = to.getBoundingClientRect();
    if (reduce || !from || target.width === 0) return Promise.resolve();
    const end = { x: target.left + 16, y: target.top };
    document.querySelectorAll('.comet-layer').forEach((layer) => layer.remove());
    const ns = 'http://www.w3.org/2000/svg';
    const layer = document.createElementNS(ns, 'svg');
    layer.setAttribute('class', 'comet-layer');
    layer.setAttribute('aria-hidden', 'true');
    layer.setAttribute('width', String(innerWidth));
    layer.setAttribute('height', String(innerHeight));
    // Дуга вверх-в-сторону, как в макете: точка не идёт по прямой поверх виджетов.
    const lift = Math.min(120, Math.abs(end.x - from.x) * 0.35 + 40);
    const c1 = { x: from.x + (end.x - from.x) * 0.25, y: Math.min(from.y, end.y) - lift };
    const c2 = { x: from.x + (end.x - from.x) * 0.7, y: Math.min(from.y, end.y) - lift * 0.4 };
    const d = `M${from.x} ${from.y}C${c1.x} ${c1.y} ${c2.x} ${c2.y} ${end.x} ${end.y}`;
    const trail = document.createElementNS(ns, 'path');
    trail.setAttribute('d', d);
    trail.setAttribute('class', 'comet-trail');
    trail.setAttribute('pathLength', '100');
    layer.appendChild(trail);
    const dot = document.createElement('span');
    dot.className = 'comet-dot';
    dot.style.offsetPath = `path("${d}")`;
    document.body.append(layer, dot);
    const easing = 'cubic-bezier(.4,0,.2,1)';
    const flight = dot.animate(
        [
            { offsetDistance: '0%', opacity: 1 },
            { offsetDistance: '92%', opacity: 1 },
            { offsetDistance: '100%', opacity: 0 },
        ],
        { duration: FLIGHT_MS, easing, fill: 'forwards' },
    );
    trail.animate([{ strokeDashoffset: 12 }, { strokeDashoffset: -100 }], { duration: FLIGHT_MS, easing, fill: 'forwards' });
    return flight.finished
        .catch(() => undefined)
        .then(() => {
            layer.remove();
            dot.remove();
        });
}
