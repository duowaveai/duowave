import '../brand-icons.css';
import { BRANDS, type BrandId } from '../brand-icons';

/**
 * Знак сервиса: Claude, Notion, Telegram… Цвет бренда, а где он не читается на теме — цвет текста. Решает это
 * scripts/brand-icons.mjs, тему переключают переменные --brand-<id> из brand-icons.css, без перерисовки.
 * Исполнителя знак не означает: кто отвечает и в каком состоянии, показывают плашка и кольцо (DESIGN.md).
 */
export function BrandMark({ id, size = 20 }: { id: BrandId; size?: number }) {
    const brand = BRANDS[id];
    return (
        <svg
            aria-hidden="true"
            width={size}
            height={size}
            viewBox={brand.viewBox ?? '0 0 24 24'}
            fill="currentColor"
            style={{ color: `var(--brand-${id}, currentColor)` }}
        >
            <path d={brand.path} fill-rule={brand.fillRule} />
        </svg>
    );
}

/** Знак в круглом значке строки — тот же круг, что у маршрутов (.route-icon); на подключениях он 38px (DESIGN.md). */
export function BrandIconCircle({ id, size = 38 }: { id: BrandId; size?: number }) {
    return (
        <span class="route-icon" aria-hidden="true" style={{ width: size, height: size }}>
            <BrandMark id={id} size={Math.round(size / 2)} />
        </span>
    );
}
