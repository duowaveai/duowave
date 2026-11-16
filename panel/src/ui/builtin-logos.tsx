// Логотипы встроенных приложений (packages/*, bundled.json) — нарисованы кодом, в одном стиле: плитка со своим
// градиентом и белый знак с одним-двумя цветными акцентами. Без картинок и без букв-заглушек (владелец, 16.11:
// «скучные»). Сетка знака — 64×64; плитку скругляет и подсвечивает `.app-icon.is-logo` (desk.css).
// Знаки без <defs> и id: на странице их много, а ссылка на градиент в скрытом элементе в WebKit пропадает.

import type { JSX } from 'preact';

type Logo = { bg: string; glyph: () => JSX.Element };

const W = '#FFFFFF';
const SOFT = 'rgba(255,255,255,0.62)';

const LOGOS: Record<string, Logo> = {
    calendar: {
        bg: 'linear-gradient(150deg, #ff9a8b 0%, #f2546a 52%, #c0243f 100%)',
        glyph: () => (
            <>
                <rect x="13" y="16" width="38" height="35" rx="8" fill={W} />
                <path d="M13 24a8 8 0 0 1 8-8h22a8 8 0 0 1 8 8v4H13z" fill="#d6304a" />
                <rect x="21" y="11" width="4.5" height="10" rx="2.25" fill={W} />
                <rect x="38.5" y="11" width="4.5" height="10" rx="2.25" fill={W} />
                {[20, 27.5, 35, 42.5].map((x) => (
                    <rect key={`a${x}`} x={x - 2.5} y="32" width="5" height="5" rx="1.6" fill="#f6c2ca" />
                ))}
                {[20, 27.5, 42.5].map((x) => (
                    <rect key={`b${x}`} x={x - 2.5} y="40.5" width="5" height="5" rx="1.6" fill="#f6c2ca" />
                ))}
                <circle cx="35" cy="43" r="4.2" fill="#e23b55" />
            </>
        ),
    },
    currency: {
        bg: 'linear-gradient(150deg, #ffe08a 0%, #f5b23d 50%, #d27a12 100%)',
        glyph: () => (
            <>
                {[44, 38, 32].map((y) => (
                    <g key={y}>
                        <path d={`M12 ${y}v5a11 4 0 0 0 22 0v-5`} fill={SOFT} />
                        <ellipse cx="23" cy={y} rx="11" ry="4" fill={W} />
                    </g>
                ))}
                <circle cx="41" cy="37" r="13" fill="#b86a0c" opacity="0.28" />
                <circle cx="40" cy="35" r="13" fill={W} />
                <circle cx="40" cy="35" r="9" fill="none" stroke="#f0a32b" stroke-width="2" />
                <path d="M40 28.5l1.9 4.6 4.6 1.9-4.6 1.9-1.9 4.6-1.9-4.6-4.6-1.9 4.6-1.9z" fill="#e8921a" />
            </>
        ),
    },
    folders: {
        bg: 'linear-gradient(150deg, #8cc8ff 0%, #4a8ff5 50%, #2453d6 100%)',
        glyph: () => (
            <>
                <path d="M11 22a5 5 0 0 1 5-5h9.5a3 3 0 0 1 2.3 1.1L31 22h17a5 5 0 0 1 5 5v4H11z" fill={SOFT} />
                <rect x="16" y="21" width="32" height="12" rx="2.5" fill="#e8f1ff" />
                <rect x="20" y="24.5" width="14" height="2.2" rx="1.1" fill="#9cc3ff" />
                <path d="M9.5 31.5a4 4 0 0 1 4-4h37a4 4 0 0 1 4 4.4l-1.6 13.2a5 5 0 0 1-5 4.4H16.1a5 5 0 0 1-5-4.4z" fill={W} />
                <rect x="26" y="36" width="12" height="3" rx="1.5" fill="#bcd6ff" />
            </>
        ),
    },
    games: {
        bg: 'linear-gradient(150deg, #ff9ad5 0%, #e046b5 50%, #8a1fb8 100%)',
        glyph: () => (
            <>
                <path d="M22 22h20c6 0 10 4 11 10l2 10c1 5-3 8-7 6l-6-4H22l-6 4c-4 2-8-1-7-6l2-10c1-6 5-10 11-10z" fill={W} />
                <rect x="16.5" y="31.5" width="11" height="3.6" rx="1.2" fill="#9b1fa6" />
                <rect x="20.2" y="27.8" width="3.6" height="11" rx="1.2" fill="#9b1fa6" />
                <circle cx="41" cy="30.5" r="2.6" fill="#ff5fae" />
                <circle cx="46" cy="35.5" r="2.6" fill="#8a3dff" />
                <circle cx="36" cy="35.5" r="2.6" fill="#ffb23d" />
                <circle cx="41" cy="40.5" r="2.6" fill="#36c6a8" />
            </>
        ),
    },
    morning: {
        bg: 'linear-gradient(165deg, #ffd27a 0%, #ff8a5b 48%, #ea4c89 100%)',
        glyph: () => (
            <>
                <path d="M18 42a14 14 0 0 1 28 0z" fill="#fff4c2" />
                <g stroke={W} stroke-width="3" stroke-linecap="round">
                    <path d="M32 24v-5" />
                    <path d="M43.6 28.2l3.2-3.8" />
                    <path d="M20.4 28.2l-3.2-3.8" />
                    <path d="M49.4 37.3l4.3-1.2" />
                    <path d="M14.6 37.3l-4.3-1.2" />
                </g>
                <rect x="10" y="43" width="44" height="3.4" rx="1.7" fill={W} />
                <rect x="17" y="49.5" width="30" height="3" rx="1.5" fill={SOFT} />
            </>
        ),
    },
    music: {
        bg: 'linear-gradient(150deg, #c3b2ff 0%, #8b6cf6 50%, #5532d1 100%)',
        glyph: () => (
            <>
                <path d="M26 21L49 15v6.5L26 27.5z" fill={W} />
                <path d="M26 30.5L49 24.5v3.6L26 34.1z" fill={SOFT} />
                <rect x="26" y="21" width="3.4" height="24" rx="1.2" fill={W} />
                <rect x="45.6" y="15" width="3.4" height="25" rx="1.2" fill={W} />
                <ellipse cx="21.5" cy="45" rx="6.6" ry="5" transform="rotate(-22 21.5 45)" fill={W} />
                <ellipse cx="41.1" cy="40" rx="6.6" ry="5" transform="rotate(-22 41.1 40)" fill={W} />
            </>
        ),
    },
    'neon-snake': {
        bg: 'radial-gradient(120% 100% at 30% 15%, #263a6b 0%, #121a36 55%, #090d1f 100%)',
        glyph: () => (
            <>
                <path d="M16 8v48M24 8v48M32 8v48M40 8v48M48 8v48M8 16h48M8 24h48M8 32h48M8 40h48M8 48h48" stroke="#ffffff" stroke-opacity="0.06" stroke-width="1" />
                <circle cx="20" cy="22" r="7" fill="#ff4fa3" opacity="0.25" />
                <circle cx="20" cy="22" r="3.6" fill="#ff6bb5" />
                <path d="M12 46H30a6 6 0 0 0 6-6v-6a6 6 0 0 1 6-6h4" fill="none" stroke="#7dff6b" stroke-opacity="0.22" stroke-width="12" stroke-linecap="round" stroke-linejoin="round" />
                <path d="M12 46H30a6 6 0 0 0 6-6v-6a6 6 0 0 1 6-6h4" fill="none" stroke="#7dff6b" stroke-width="6" stroke-linecap="round" stroke-linejoin="round" />
                <circle cx="48" cy="28" r="5.6" fill="#b6ffab" />
                <circle cx="49.6" cy="26.4" r="1.4" fill="#0b1226" />
            </>
        ),
    },
    news: {
        bg: 'linear-gradient(150deg, #b7c3d4 0%, #74849c 50%, #3f4b60 100%)',
        glyph: () => (
            <>
                <rect x="38" y="21" width="15" height="28" rx="4" fill={SOFT} />
                <rect x="11" y="15" width="35" height="34" rx="5.5" fill={W} />
                <rect x="16" y="20.5" width="25" height="4.4" rx="2.2" fill="#3f4b60" />
                <rect x="16" y="29" width="10.5" height="9.5" rx="2" fill="#8fa0b8" />
                <rect x="29.5" y="29" width="11.5" height="2.6" rx="1.3" fill="#a9b5c6" />
                <rect x="29.5" y="33" width="11.5" height="2.6" rx="1.3" fill="#a9b5c6" />
                <rect x="29.5" y="37" width="8" height="2.6" rx="1.3" fill="#a9b5c6" />
                <rect x="16" y="42" width="25" height="2.6" rx="1.3" fill="#cfd7e2" />
            </>
        ),
    },
    radio: {
        bg: 'linear-gradient(150deg, #7ff0dc 0%, #22b8b0 50%, #0b6f86 100%)',
        glyph: () => (
            <>
                <path d="M21 26L41 14.5" stroke={W} stroke-width="3" stroke-linecap="round" />
                <circle cx="41.5" cy="14.2" r="2.8" fill={W} />
                <rect x="11" y="25" width="42" height="26" rx="7" fill={W} />
                <circle cx="24" cy="38" r="8" fill="#0d7c8c" />
                <circle cx="24" cy="38" r="3.4" fill="#6ff0dc" />
                <rect x="36" y="31.5" width="12" height="2.8" rx="1.4" fill="#9adfe0" />
                <rect x="36" y="36.2" width="12" height="2.8" rx="1.4" fill="#9adfe0" />
                <circle cx="42" cy="44.2" r="2.8" fill="#0d7c8c" />
            </>
        ),
    },
    shopping: {
        bg: 'linear-gradient(150deg, #a6f3a0 0%, #3fcf6d 50%, #148a4b 100%)',
        glyph: () => (
            <>
                <rect x="14" y="11" width="36" height="42" rx="7" fill={W} />
                {[19, 30].map((y) => (
                    <g key={y}>
                        <rect x="19" y={y} width="8" height="8" rx="2.4" fill="#22a95a" />
                        <path d={`M21 ${y + 4.2}l1.9 1.9 3.3-3.7`} fill="none" stroke={W} stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
                        <rect x="31" y={y + 2.6} width="13" height="2.8" rx="1.4" fill="#bdebc9" />
                    </g>
                ))}
                <rect x="19.9" y="41.9" width="6.2" height="6.2" rx="1.8" fill="none" stroke="#22a95a" stroke-width="1.8" />
                <rect x="31" y="43.6" width="13" height="2.8" rx="1.4" fill="#1d8f4d" />
            </>
        ),
    },
    'smart-home': {
        bg: 'linear-gradient(150deg, #ffc58a 0%, #ff8540 50%, #d9461c 100%)',
        glyph: () => (
            <>
                <path d="M30.1 12.5a3 3 0 0 1 3.8 0l18 14.6a2 2 0 0 1-1.3 3.6H48V47a4 4 0 0 1-4 4H20a4 4 0 0 1-4-4V30.7h-2.6a2 2 0 0 1-1.3-3.6z" fill={W} />
                <path d="M28.5 40.5a5 5 0 0 1 7 0M25.3 37.3a9.5 9.5 0 0 1 13.4 0" fill="none" stroke="#f26a2a" stroke-width="2.8" stroke-linecap="round" />
                <circle cx="32" cy="44.2" r="2.4" fill="#f26a2a" />
            </>
        ),
    },
    tales: {
        bg: 'linear-gradient(160deg, #8e83ff 0%, #5a45d8 48%, #2a1f7a 100%)',
        glyph: () => (
            <>
                <path d="M33.5 9.5a6.5 6.5 0 1 0 5.5 10.2 5.3 5.3 0 0 1-5.5-10.2z" fill="#ffe08a" />
                <path d="M19 13.5l1 2.5 2.5 1-2.5 1-1 2.5-1-2.5-2.5-1 2.5-1z" fill="#ffe08a" />
                <path d="M46.5 16l.8 1.9 1.9.8-1.9.8-.8 1.9-.8-1.9-1.9-.8 1.9-.8z" fill="#ffe08a" />
                <path d="M32 51C26 47 19 46 11 47V27c8-1 15 0 21 4z" fill={W} />
                <path d="M32 51c6-4 13-5 21-4V27c-8-1-15 0-21 4z" fill="#e4e0ff" />
                <path d="M15.5 33.5c4-.4 8 .2 12 2M15.5 39c4-.4 8 .2 12 2M48.5 33.5c-4-.4-8 .2-12 2M48.5 39c-4-.4-8 .2-12 2" fill="none" stroke="#8d82e6" stroke-width="1.6" stroke-linecap="round" />
            </>
        ),
    },
    weather: {
        bg: 'linear-gradient(150deg, #9be3ff 0%, #44a8f0 50%, #1f68cf 100%)',
        glyph: () => (
            <>
                <g stroke="#ffd54a" stroke-width="2.8" stroke-linecap="round">
                    <path d="M24 11V8" />
                    <path d="M32.5 14.5l2.1-2.1" />
                    <path d="M15.5 14.5l-2.1-2.1" />
                    <path d="M12 23H9" />
                    <path d="M36 23h3" />
                </g>
                <circle cx="24" cy="23" r="8.5" fill="#ffd54a" />
                <path d="M20 49a8 8 0 0 1 0-16 11 11 0 0 1 21-3 9.5 9.5 0 1 1 3 19z" fill="#1f68cf" opacity="0.18" transform="translate(1 2)" />
                <path d="M20 49a8 8 0 0 1 0-16 11 11 0 0 1 21-3 9.5 9.5 0 1 1 3 19z" fill={W} />
            </>
        ),
    },
};

/** Есть ли у приложения свой нарисованный логотип (встроенные приложения). */
export function hasBuiltinLogo(id: string): boolean {
    return Object.prototype.hasOwnProperty.call(LOGOS, id.replace(/^package\./, ''));
}

export function BuiltinLogo({ id, size }: { id: string; size: number }) {
    const logo = LOGOS[id.replace(/^package\./, '')];
    if (!logo) return null;
    return (
        <span class="app-icon is-logo" style={{ width: `${size}px`, height: `${size}px`, background: logo.bg }} aria-hidden="true">
            <svg viewBox="0 0 64 64" width={size} height={size}>
                {logo.glyph()}
            </svg>
        </span>
    );
}
