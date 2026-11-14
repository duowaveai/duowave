// Темы пульта — данные, а не код: палитры двух режимов, плотность, шрифт и скругления. Тема одна — «Минимал»
// (docs/plans/desk.md); темы пакетов перекрашивают её. Вид (стол и док или боковая панель) выбирает человек отдельно
// от темы (theme.ts). Встроенную тему регистрирует тот же definePlugin, что зовут плагины (builtin.tsx).
// Файл чистый — без DOM и Preact: его читает и vite.config.ts, чтобы вшить тему по умолчанию в index.html. Поэтому подписи
// здесь русские и без словаря: на язык их переводит пульт по `id` темы (shell.themeNames), см. themeTitle в theme-picker.tsx.

/** Режим внутри темы: тёмный или светлый. */
export type Mode = 'dark' | 'light';

/**
 * Палитра одного режима: CSS-токены без «--» — те, что читают стили и экраны (bg, text, accent, pill-own-fg…).
 * Тема-пакет приносит только то, что меняет: остальное берётся у «Минимала».
 */
export type Palette = Record<string, string>;

export interface Theme {
    id: string;
    title: string;
    /** Одна строка в выборе темы: чем она отличается. */
    about: string;
    /** Подписи режимов в выборе темы. */
    modes: Record<Mode, string>;
    /** Строка под выбором режима, если режиму есть что сказать о себе (тёмный «Минимал» — производная светлого). */
    modeNotes?: Partial<Record<Mode, string>>;
    /** Режим, пока человек не выбрал свой; без него — как у системы. */
    defaultMode?: Mode;
    palette: Record<Mode, Palette>;
    /**
     * Материал — всегда 'minimal' (плоские тёплые карточки с линией, капсулы и белые листы; DESIGN.md, «Минимал»):
     * «Стекло» удалено 10.11.2026. Поле осталось для пакетов, которые его пишут; ставится на `<html data-look>`.
     */
    look?: string;
    /**
     * Вид, который тема советует: 'desk' — стол и док, 'sidebar' — боковая панель ('tabbar' прежних пакетов —
     * стол). Вид выбирает человек (theme.ts), а тема его не навязывает.
     */
    layout: string;
    /** Плотность — множитель отступов: 1 — как в DESIGN.md, больше — просторнее. Высоту нажимаемых целей не трогает. */
    density: number;
    /** Шрифт интерфейса — CSS font-family. Файлы шрифта приносит тот, кто приносит тему. */
    font: string;
    /** Скругления, px: панели и карточки, большие поверхности (шит, боковая панель), плавающая панель вкладок. */
    radius: { panel: number; large: number; bar: number };
}

/**
 * «Минимал» — стиль нового duowave.app (landing-design/minimal, плашки С1–С6): белый фон без сияния и градиентов,
 * тонкие линии, светло-серые карточки #FAFAFA, белые листы и плитки, капсулы; лайм — только у главного действия. Заголовки
 * Inter Tight 300, текст Inter 400/500, код JetBrains Mono (ui/minimal.css).
 * Цвет исполнителя живёт в точках статусов, кольце колонки, свечении за ней и кометах: лайм — своё, янтарь — снаружи,
 * фиолет — агент, серый — выключено. Тёмный режим — сдержанная производная того же вида, не стекло: макета у него нет.
 * Видов два, их выбирает человек: стол и док (основной) и боковая панель (theme.ts).
 */
export const MINIMAL: Theme = {
    id: 'minimal',
    title: 'Минимал',
    about: 'Тонкие линии, светлые карточки, лайм — только у «Сказать»',
    modes: { dark: 'Тёмный', light: 'Светлый' },
    look: 'minimal',
    layout: 'desk',
    density: 1,
    font: "'Inter Panel', 'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif",
    radius: { panel: 16, large: 20, bar: 20 },
    palette: {
        light: {
            // Основа: белый фон без сияния (решение владельца 08.11.2026), текст, вторичный 56 %, приглушённый 38 %
            bg: '#FFFFFF',
            aurora: 'none',
            text: '#000000',
            sub: 'rgba(0, 0, 0, 0.56)',
            mute: 'rgba(0, 0, 0, 0.38)',
            // Карточка — светло-серая #FAFAFA (решение владельца 08.11.2026) с линией, без стекла: блика, тени нет
            'glass-bg': '#FAFAFA',
            'glass-border': '#ECE9E6',
            'glass-hi': 'transparent',
            'glass-shadow': '0 0 0 rgba(0, 0, 0, 0)',
            'glass-blur': '0px',
            // То, что плавает (панель вкладок, боковая панель, шит), — белый лист с линией и мягкой тенью
            'strong-bg': '#FFFFFF',
            'strong-border': '#ECE9E6',
            'strong-hi': 'transparent',
            'strong-shadow': '0 10px 40px rgba(0, 0, 0, 0.05)',
            'strong-blur': '0px',
            'menu-bg': '#FFFFFF',
            divider: '#ECE9E6',
            'row-line': '#F1EFED',
            track: '#EFECE9',
            'icon-bg': '#FFFFFF',
            'chip-bg': '#F3F1EF',
            'field-bg': '#FFFFFF',
            'hover-bg': '#FAF9F8',
            // Акцент — лайм, только у главного действия; текст лаймового смысла — глубокий оливковый
            accent: '#B6F36A',
            'accent-hi': '#A3E635',
            'on-accent': '#0B0F0A',
            'accent-text': '#3F6212',
            'talk-shadow': '0 6px 18px rgba(132, 204, 22, 0.32)',
            'talk-shadow-hi': '0 8px 26px rgba(132, 204, 22, 0.46)',
            // Вторая кнопка — чёрная
            'ink-bg': '#000000',
            'ink-hi': '#262626',
            'ink-fg': '#FFFFFF',
            // Исполнители (С1, статусы): работает — лайм, снаружи — янтарь, агент — фиолет, нет — камень
            'bar-own': '#84CC16',
            'bar-yandex': '#F59E0B',
            'bar-agent': '#8B5CF6',
            'bar-off': '#A8A29E',
            'pill-own-fg': '#3F6212',
            'pill-own-bg': '#EEF8DF',
            'pill-yandex-fg': '#92400E',
            'pill-yandex-bg': '#FCF1DE',
            'pill-agent-fg': '#5B21B6',
            'pill-agent-bg': '#F1ECFE',
            'pill-off-fg': 'rgba(0, 0, 0, 0.56)',
            'pill-off-bg': '#F3F1EF',
            'dot-offline': '#C8321F',
            // Слои памяти (окно «Память»): дом — бирюзовый, личное персонажа — розовый, из пакета — синий с замком.
            // Лайм, янтарь и фиолет не берутся: они значат «кто отвечает».
            'mem-home-fg': '#0F5F59',
            'mem-home-bg': '#E2F2EE',
            'mem-pers-fg': '#9F1239',
            'mem-pers-bg': '#FBEAF0',
            'mem-pkg-fg': '#1D4E89',
            'mem-pkg-bg': '#E8EFF8',
            // Выбор — белая капсула с тенью на тёплой дорожке (вкладки панели, сегменты)
            'seg-on-bg': '#FFFFFF',
            'seg-on-fg': '#000000',
            'seg-off-fg': 'rgba(0, 0, 0, 0.56)',
            'seg-shadow': '0 1px 3px rgba(0, 0, 0, 0.08)',
            'radio-on-bg': '#000000',
            'radio-on-fg': '#FFFFFF',
            'radio-off': 'rgba(0, 0, 0, 0.2)',
            // Сцена дашборда: подписи и узлы — белые, орбита — тонкая тёплая линия, свечение за колонкой
            'label-bg': '#FFFFFF',
            'label-border': '#ECE9E6',
            'label-hi': 'transparent',
            'label-shadow': '0 2px 10px rgba(0, 0, 0, 0.04)',
            'node-hi': 'transparent',
            'node-shadow': '0 4px 14px rgba(0, 0, 0, 0.06)',
            'soft-own': 'rgba(132, 204, 22, 0.45)',
            'soft-yandex': 'rgba(245, 158, 11, 0.45)',
            'soft-agent': 'rgba(139, 92, 246, 0.4)',
            'soft-off': 'rgba(168, 162, 158, 0.4)',
            'on-badge': '#FFFFFF',
            orbit: '#E2DDD8',
            'orbit-hi': '#84CC16',
            'glow-rest': '#E7E5E4',
            'glow-opacity': '0.45',
            scrim: 'rgba(28, 25, 23, 0.18)',
            grabber: '#E2DDD8',
            'station-shadow': '0.22',
        },
        dark: {
            // Тот же вид в сумерках: тёплый почти чёрный фон, карточки на тон светлее, линии — ещё на тон
            bg: '#11100F',
            aurora: 'none',
            text: '#F5F3F1',
            sub: 'rgba(245, 243, 241, 0.6)',
            mute: 'rgba(245, 243, 241, 0.4)',
            'glass-bg': '#1A1918',
            'glass-border': '#2A2826',
            'glass-hi': 'transparent',
            'glass-shadow': '0 0 0 rgba(0, 0, 0, 0)',
            'glass-blur': '0px',
            'strong-bg': '#201F1D',
            'strong-border': '#2E2C2A',
            'strong-hi': 'transparent',
            'strong-shadow': '0 10px 40px rgba(0, 0, 0, 0.35)',
            'strong-blur': '0px',
            'menu-bg': '#201F1D',
            divider: '#2A2826',
            'row-line': '#262422',
            track: '#262422',
            'icon-bg': '#242220',
            'chip-bg': '#262422',
            'field-bg': '#151413',
            'hover-bg': '#242220',
            accent: '#B6F36A',
            'accent-hi': '#C5F78A',
            'on-accent': '#0B0F0A',
            'accent-text': '#B6F36A',
            'talk-shadow': '0 6px 18px rgba(132, 204, 22, 0.22)',
            'talk-shadow-hi': '0 8px 26px rgba(132, 204, 22, 0.32)',
            'ink-bg': '#F5F3F1',
            'ink-hi': '#FFFFFF',
            'ink-fg': '#11100F',
            'bar-own': '#A3E635',
            'bar-yandex': '#FBBF24',
            'bar-agent': '#A78BFA',
            'bar-off': '#8A8580',
            'pill-own-fg': '#BEF264',
            'pill-own-bg': 'rgba(163, 230, 53, 0.12)',
            'pill-yandex-fg': '#FCD34D',
            'pill-yandex-bg': 'rgba(251, 191, 36, 0.12)',
            'pill-agent-fg': '#C4B5FD',
            'pill-agent-bg': 'rgba(167, 139, 250, 0.14)',
            'pill-off-fg': 'rgba(245, 243, 241, 0.6)',
            'pill-off-bg': '#262422',
            'dot-offline': '#FF8A75',
            'mem-home-fg': '#8FD8CA',
            'mem-home-bg': 'rgba(45, 212, 191, 0.12)',
            'mem-pers-fg': '#F3A8BD',
            'mem-pers-bg': 'rgba(244, 114, 182, 0.12)',
            'mem-pkg-fg': '#A9C8F2',
            'mem-pkg-bg': 'rgba(96, 165, 250, 0.12)',
            'seg-on-bg': '#33312E',
            'seg-on-fg': '#F5F3F1',
            'seg-off-fg': 'rgba(245, 243, 241, 0.6)',
            'seg-shadow': '0 1px 3px rgba(0, 0, 0, 0.4)',
            'radio-on-bg': '#F5F3F1',
            'radio-on-fg': '#11100F',
            'radio-off': 'rgba(245, 243, 241, 0.26)',
            'label-bg': '#201F1D',
            'label-border': '#2E2C2A',
            'label-hi': 'transparent',
            'label-shadow': '0 2px 10px rgba(0, 0, 0, 0.3)',
            'node-hi': 'transparent',
            'node-shadow': '0 4px 14px rgba(0, 0, 0, 0.4)',
            'soft-own': 'rgba(163, 230, 53, 0.4)',
            'soft-yandex': 'rgba(251, 191, 36, 0.4)',
            'soft-agent': 'rgba(167, 139, 250, 0.4)',
            'soft-off': 'rgba(138, 133, 128, 0.4)',
            'on-badge': '#11100F',
            orbit: '#3A3734',
            'orbit-hi': '#A3E635',
            'glow-rest': '#3A3734',
            'glow-opacity': '0.32',
            scrim: 'rgba(0, 0, 0, 0.5)',
            grabber: '#3A3734',
            'station-shadow': '0.5',
        },
    },
};

/** Тема, которая стоит, пока человек не выбрал другую, и на которую пульт откатывается, если выбранной нет. */
export const DEFAULT_THEME = MINIMAL;

/** Значение для CSS: тема — чужие данные (плагин), поэтому `;`, `{`, `}` и `<` не пропускаем — правило не сломать. */
const safe = (value: string): string => value.replace(/[;{}<]/g, '');

/** Объявления темы в одном режиме: color-scheme, палитра, шрифт, плотность и скругления — всё CSS-токенами. */
export function themeDeclarations(theme: Theme, mode: Mode): string {
    const tokens: Record<string, string> = {
        ...theme.palette[mode],
        font: theme.font,
        density: String(theme.density),
        'r-panel': `${theme.radius.panel}px`,
        'r-panel-desk': `${theme.radius.large}px`,
        'r-bar': `${theme.radius.bar}px`,
    };
    const lines = Object.entries(tokens)
        .filter(([name]) => /^[a-z][a-z0-9-]*$/.test(name))
        .map(([name, value]) => `--${name}: ${safe(value)};`);
    return [`color-scheme: ${mode};`, ...lines].join(' ');
}

/**
 * Тема по умолчанию для index.html — до загрузки скриптов, чтобы первая отрисовка была в цвет: тёмный режим на
 * :root, светлый — по data-theme. Выбранная тема (theme.ts) перекрывает её правилом посильнее.
 */
export function bootCss(theme: Theme): string {
    return `:root { ${themeDeclarations(theme, 'dark')} }\n:root[data-theme='light'] { ${themeDeclarations(theme, 'light')} }`;
}

/**
 * Что знать загрузчику в index.html о теме по умолчанию: её id, режим, если он у неё свой, и материал — атрибутами
 * на <style id="theme-boot">. Значения проверены здесь, в разметку они попадают как есть.
 */
export function bootAttributes(theme: Theme): string {
    const mode = theme.defaultMode ? ` data-mode="${theme.defaultMode}"` : '';
    return `data-id="${theme.id.replace(/[^a-z0-9-]/g, '')}" data-look="minimal"${mode}`;
}

/** Правило выбранной темы: специфичнее bootCss, поэтому побеждает его независимо от порядка в документе. */
export function activeCss(theme: Theme, mode: Mode): string {
    return `html:root[data-theme] { ${themeDeclarations(theme, mode)} }`;
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);

/** Только строковые токены с именами из строчных букв, цифр и «-» — остальное тема-пакет не приносит. */
function paletteOf(value: unknown): Palette {
    const palette: Palette = {};
    if (!isRecord(value)) return palette;
    for (const [token, tokenValue] of Object.entries(value)) {
        if (/^[a-z][a-z0-9-]*$/.test(token) && typeof tokenValue === 'string') palette[token] = tokenValue;
    }
    return palette;
}

const clamp = (value: unknown, min: number, max: number, fallback: number): number =>
    typeof value === 'number' && Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;

const isMode = (value: unknown): value is Mode => value === 'dark' || value === 'light';

/**
 * Тема из пакета (kind: theme или плагин с contributes.themes) — чужие данные, поэтому каждое поле проверяется, а чего
 * нет, берётся у «Минимала»: палитра пакета ложится поверх его палитры. Материал — всегда «Минимал» (пакет, который
 * просил «Стекло», получает его). Вид — только известный: 'sidebar' или стол (THEMES.md).
 */
export function normalizeTheme(raw: unknown): Theme | null {
    if (!isRecord(raw)) return null;
    const id = typeof raw.id === 'string' && /^[a-z0-9-]{1,40}$/.test(raw.id) ? raw.id : null;
    const title = typeof raw.title === 'string' ? raw.title.trim().slice(0, 40) : '';
    if (!id || !title || !isRecord(raw.palette)) return null;
    const base = MINIMAL;
    const modes = isRecord(raw.modes) ? raw.modes : {};
    const notes = isRecord(raw.modeNotes) ? raw.modeNotes : {};
    const radius = isRecord(raw.radius) ? raw.radius : {};
    const modeNotes: Partial<Record<Mode, string>> = {};
    for (const mode of ['dark', 'light'] as const) if (typeof notes[mode] === 'string') modeNotes[mode] = (notes[mode] as string).slice(0, 160);
    return {
        id,
        title,
        about: typeof raw.about === 'string' ? raw.about.slice(0, 120) : '',
        modes: {
            dark: typeof modes.dark === 'string' ? modes.dark.slice(0, 20) : base.modes.dark,
            light: typeof modes.light === 'string' ? modes.light.slice(0, 20) : base.modes.light,
        },
        modeNotes,
        defaultMode: isMode(raw.defaultMode) ? raw.defaultMode : undefined,
        look: 'minimal',
        palette: {
            dark: { ...base.palette.dark, ...paletteOf(raw.palette.dark) },
            light: { ...base.palette.light, ...paletteOf(raw.palette.light) },
        },
        layout: raw.layout === 'sidebar' ? 'sidebar' : 'desk',
        density: clamp(raw.density, 0.8, 1.5, 1),
        font: typeof raw.font === 'string' && raw.font.trim() ? raw.font.slice(0, 200) : base.font,
        radius: {
            panel: clamp(radius.panel, 0, 48, base.radius.panel),
            large: clamp(radius.large, 0, 56, base.radius.large),
            bar: clamp(radius.bar, 0, 48, base.radius.bar),
        },
    };
}
