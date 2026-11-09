// Темы пульта — данные, а не код: палитры двух режимов, оболочка, плотность, шрифт и скругления. Встроенные темы
// регистрирует тот же definePlugin, что потом будут звать плагины (builtin.tsx); выбирает и применяет тему theme.ts.
// Файл чистый — без DOM и Preact: его читает и vite.config.ts, чтобы вшить тему по умолчанию в index.html.

/** Режим внутри темы. У «Стекла» тёмный — «ночь», светлый — «туман». */
export type Mode = 'dark' | 'light';

/**
 * Палитра одного режима: CSS-токены без «--» — те, что читают glass.css и экраны (bg, text, accent, pill-own-fg…).
 * Плагину-теме проще всего взять палитру «Стекла» и поменять нужное: `{ ...GLASS.palette.dark, bg: '#…' }`.
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
     * Материал: как выглядят поверхности и элементы. 'glass' — стекло (DESIGN.md), 'minimal' — плоские тёплые
     * карточки с линией, капсулы и белые листы (DESIGN.md, «Минимал»). Ставится на `<html data-look>`; незнакомый —
     * стекло. Палитра темы ложится поверх палитры своего материала.
     */
    look?: string;
    /**
     * Оболочка: где навигация и шапка. 'tabbar' — панель вкладок снизу, 'sidebar' — боковая панель. Незнакомая
     * оболочка — панель вкладок.
     */
    layout: string;
    /** Плотность — множитель отступов: 1 — как в DESIGN.md, больше — просторнее. Высоту нажимаемых целей не трогает. */
    density: number;
    /** Шрифт интерфейса — CSS font-family. Файлы шрифта приносит тот, кто приносит тему. */
    font: string;
    /** Скругления, px: панели и карточки, большие поверхности (шит, боковая панель), плавающая панель вкладок. */
    radius: { panel: number; large: number; bar: number };
}

/** «Стекло» (DESIGN.md): тёмная «ночь» и светлый «туман» — токены один в один из прежнего glass.css. */
export const GLASS: Theme = {
    id: 'glass',
    title: 'Стекло',
    about: 'Панель вкладок снизу, кнопка «Сказать» посередине',
    modes: { dark: 'Ночь', light: 'Туман' },
    layout: 'tabbar',
    density: 1,
    font: "'Manrope', system-ui, sans-serif",
    radius: { panel: 24, large: 32, bar: 34 },
    palette: {
        dark: {
            // Текст и база
            bg: '#090C10',
            aurora:
                'radial-gradient(120% 70% at 0% 0%, rgba(182, 243, 106, 0.30) 0%, rgba(182, 243, 106, 0) 55%), ' +
                'radial-gradient(90% 60% at 100% 38%, rgba(63, 224, 197, 0.24) 0%, rgba(63, 224, 197, 0) 60%), ' +
                'radial-gradient(110% 60% at 20% 100%, rgba(139, 108, 255, 0.26) 0%, rgba(139, 108, 255, 0) 62%)',
            text: '#F3F6F8',
            sub: 'rgba(243, 246, 248, 0.68)',
            // Стекло
            'glass-bg': 'rgba(255, 255, 255, 0.07)',
            'glass-border': 'rgba(255, 255, 255, 0.13)',
            'glass-hi': 'rgba(255, 255, 255, 0.10)',
            'glass-shadow': '0 12px 32px rgba(0, 0, 0, 0.35)',
            'glass-blur': '24px',
            'strong-bg': 'rgba(16, 20, 27, 0.62)',
            'strong-border': 'rgba(255, 255, 255, 0.14)',
            'strong-hi': 'rgba(255, 255, 255, 0.10)',
            'strong-shadow': '0 16px 40px rgba(0, 0, 0, 0.45)',
            'strong-blur': '28px',
            // Меню списков: почти плотное — под ним не должен просвечивать текст карточек
            'menu-bg': 'rgba(14, 18, 24, 0.96)',
            divider: 'rgba(255, 255, 255, 0.08)',
            track: 'rgba(255, 255, 255, 0.14)',
            'icon-bg': 'rgba(255, 255, 255, 0.09)',
            // Акцент (лайм)
            accent: '#B6F36A',
            'on-accent': '#0B0F0A',
            'accent-text': '#B6F36A',
            'talk-shadow': '0 8px 24px rgba(182, 243, 106, 0.35)',
            // Полосы распределения
            'bar-own': '#B6F36A',
            'bar-yandex': '#FFB547',
            'bar-agent': '#C2A8FF',
            'bar-off': '#AEB6C1',
            // Пилюли исполнителей: цвет текста / заливка
            'pill-own-fg': '#B6F36A',
            'pill-own-bg': 'rgba(182, 243, 106, 0.16)',
            'pill-yandex-fg': '#FFB547',
            'pill-yandex-bg': 'rgba(255, 181, 71, 0.16)',
            'pill-agent-fg': '#C2A8FF',
            'pill-agent-bg': 'rgba(183, 155, 255, 0.18)',
            'pill-off-fg': '#AEB6C1',
            'pill-off-bg': 'rgba(174, 182, 193, 0.14)',
            'dot-offline': '#FF8A80',
            // Выбранный сегмент и радио: лайм в тёмной теме, чернила в светлой (DESIGN.md: The Ink-Selection Rule)
            'seg-on-bg': '#B6F36A',
            'seg-on-fg': '#0B0F0A',
            'seg-off-fg': 'rgba(243, 246, 248, 0.72)',
            'radio-on-bg': '#B6F36A',
            'radio-on-fg': '#0B0F0A',
            'radio-off': 'rgba(255, 255, 255, 0.3)',
            // Дашборд: подписи и узлы орбиты, сама орбита, затемнение шита
            'label-bg': 'rgba(16, 20, 27, 0.5)',
            'label-border': 'rgba(255, 255, 255, 0.10)',
            'label-hi': 'rgba(255, 255, 255, 0.08)',
            'label-shadow': '0 6px 18px rgba(0, 0, 0, 0.28)',
            'node-hi': 'rgba(255, 255, 255, 0.12)',
            'node-shadow': '0 10px 24px rgba(0, 0, 0, 0.35)',
            'soft-own': 'rgba(182, 243, 106, 0.42)',
            'soft-yandex': 'rgba(255, 181, 71, 0.42)',
            'soft-agent': 'rgba(194, 168, 255, 0.42)',
            'soft-off': 'rgba(174, 182, 193, 0.30)',
            'on-badge': '#0B0F0A',
            orbit: 'rgba(255, 255, 255, 0.16)',
            'orbit-hi': '#B6F36A',
            scrim: 'rgba(4, 6, 9, 0.55)',
            grabber: 'rgba(255, 255, 255, 0.18)',
            'station-shadow': '0.5',
        },
        light: {
            bg: '#EDF1EF',
            aurora:
                'radial-gradient(120% 70% at 0% 0%, rgba(163, 230, 53, 0.45) 0%, rgba(163, 230, 53, 0) 55%), ' +
                'radial-gradient(90% 60% at 100% 38%, rgba(45, 212, 191, 0.34) 0%, rgba(45, 212, 191, 0) 60%), ' +
                'radial-gradient(110% 60% at 20% 100%, rgba(167, 139, 250, 0.34) 0%, rgba(167, 139, 250, 0) 62%)',
            text: '#0D1117',
            sub: 'rgba(13, 17, 23, 0.66)',
            'glass-bg': 'rgba(255, 255, 255, 0.58)',
            'glass-border': 'rgba(255, 255, 255, 0.85)',
            'glass-hi': 'rgba(255, 255, 255, 0.9)',
            'glass-shadow': '0 10px 30px rgba(30, 41, 59, 0.10)',
            'glass-blur': '24px',
            'strong-bg': 'rgba(255, 255, 255, 0.74)',
            'strong-border': 'rgba(255, 255, 255, 0.9)',
            'strong-hi': 'rgba(255, 255, 255, 0.95)',
            'strong-shadow': '0 14px 36px rgba(30, 41, 59, 0.16)',
            'strong-blur': '28px',
            'menu-bg': 'rgba(255, 255, 255, 0.97)',
            divider: 'rgba(13, 17, 23, 0.07)',
            track: 'rgba(13, 17, 23, 0.10)',
            'icon-bg': 'rgba(255, 255, 255, 0.75)',
            accent: '#A3E635',
            'on-accent': '#0D1117',
            'accent-text': '#3F6212',
            'talk-shadow': '0 8px 22px rgba(101, 163, 13, 0.30)',
            'bar-own': '#84CC16',
            'bar-yandex': '#F59E0B',
            'bar-agent': '#8B5CF6',
            'bar-off': '#9CA3AF',
            'pill-own-fg': '#3F6212',
            'pill-own-bg': '#E2F7C2',
            'pill-yandex-fg': '#9A4A05',
            'pill-yandex-bg': '#FDE8C8',
            'pill-agent-fg': '#5B21B6',
            'pill-agent-bg': '#ECE5FF',
            'pill-off-fg': '#4B5563',
            'pill-off-bg': '#E6E8EC',
            'dot-offline': '#B42318',
            'seg-on-bg': '#0D1117',
            'seg-on-fg': '#F3F6F8',
            'seg-off-fg': 'rgba(13, 17, 23, 0.7)',
            'radio-on-bg': '#0D1117',
            'radio-on-fg': '#FFFFFF',
            'radio-off': 'rgba(13, 17, 23, 0.26)',
            'label-bg': 'rgba(255, 255, 255, 0.66)',
            'label-border': 'rgba(255, 255, 255, 0.9)',
            'label-hi': 'rgba(255, 255, 255, 0.95)',
            'label-shadow': '0 6px 16px rgba(30, 41, 59, 0.10)',
            'node-hi': 'rgba(255, 255, 255, 0.9)',
            'node-shadow': '0 8px 20px rgba(30, 41, 59, 0.14)',
            'soft-own': 'rgba(101, 163, 13, 0.45)',
            'soft-yandex': 'rgba(217, 119, 6, 0.42)',
            'soft-agent': 'rgba(124, 58, 237, 0.35)',
            'soft-off': 'rgba(75, 85, 99, 0.28)',
            'on-badge': '#FFFFFF',
            orbit: 'rgba(13, 17, 23, 0.16)',
            'orbit-hi': '#65A30D',
            scrim: 'rgba(13, 17, 23, 0.24)',
            grabber: 'rgba(13, 17, 23, 0.14)',
            'station-shadow': '0.22',
        },
    },
};

/**
 * «Простор» — то же «Стекло» для большого экрана: боковая панель вместо вкладок снизу (на телефоне она выезжает
 * слева), просторнее и круглее, стекло тоньше, как у компьютера в DESIGN.md. Сияние стоит за панелью: фиолет слева,
 * бирюза справа сверху, лайм — тише, справа снизу. Цвета исполнителей и лаймовое действие — те же: смысл цвета
 * в данных у всех тем один (The Who-Answers Rule).
 */
export const PROSTOR: Theme = {
    id: 'prostor',
    title: 'Простор',
    about: 'Боковая панель, больше воздуха — для компьютера; на телефоне разделы в меню',
    modes: { dark: 'Сумрак', light: 'Иней' },
    layout: 'sidebar',
    density: 1.125,
    font: GLASS.font,
    radius: { panel: 28, large: 32, bar: 30 },
    palette: {
        dark: {
            ...GLASS.palette.dark,
            bg: '#080B12',
            aurora:
                'radial-gradient(70% 80% at 0% 35%, rgba(139, 108, 255, 0.30) 0%, rgba(139, 108, 255, 0) 60%), ' +
                'radial-gradient(70% 55% at 100% 0%, rgba(63, 224, 197, 0.22) 0%, rgba(63, 224, 197, 0) 60%), ' +
                'radial-gradient(80% 60% at 90% 100%, rgba(182, 243, 106, 0.16) 0%, rgba(182, 243, 106, 0) 62%)',
            'glass-bg': 'rgba(255, 255, 255, 0.06)',
            'glass-shadow': '0 16px 40px rgba(0, 0, 0, 0.35)',
            'glass-blur': '28px',
            'strong-bg': 'rgba(14, 18, 28, 0.66)',
            'strong-shadow': '0 16px 40px rgba(0, 0, 0, 0.40)',
            'menu-bg': 'rgba(12, 16, 24, 0.96)',
            scrim: 'rgba(4, 6, 12, 0.55)',
        },
        light: {
            ...GLASS.palette.light,
            bg: '#EEF2F6',
            aurora:
                'radial-gradient(70% 80% at 0% 35%, rgba(167, 139, 250, 0.34) 0%, rgba(167, 139, 250, 0) 60%), ' +
                'radial-gradient(70% 55% at 100% 0%, rgba(45, 212, 191, 0.30) 0%, rgba(45, 212, 191, 0) 60%), ' +
                'radial-gradient(80% 60% at 90% 100%, rgba(163, 230, 53, 0.30) 0%, rgba(163, 230, 53, 0) 62%)',
            'glass-bg': 'rgba(255, 255, 255, 0.56)',
            'glass-shadow': '0 12px 34px rgba(30, 41, 59, 0.10)',
            'glass-blur': '28px',
            'strong-shadow': '0 14px 36px rgba(30, 41, 59, 0.14)',
        },
    },
};

/**
 * «Минимал» — стиль нового duowave.app (landing-design/minimal, плашки С1–С6): белый фон без сияния и градиентов,
 * тонкие линии, светло-серые карточки #FAFAFA, белые листы и плитки, капсулы; лайм — только у главного действия. Заголовки
 * Inter Tight 300, текст Inter 400/500, код JetBrains Mono (ui/minimal.css).
 * Цвет исполнителя живёт в точках статусов, кольце колонки, свечении за ней и кометах: лайм — своё, янтарь — снаружи,
 * фиолет — агент, серый — выключено. Тёмный режим — сдержанная производная того же вида, не стекло: макета у него нет.
 * Два вида, как у «Стекла» и «Простора»: панель вкладок снизу (эта тема) и боковая панель (MINIMAL_PANEL).
 */
export const MINIMAL: Theme = {
    id: 'minimal',
    title: 'Минимал',
    about: 'Тонкие линии и светлые карточки; панель вкладок снизу, кнопка «Сказать» посередине',
    modes: { dark: 'Тёмный', light: 'Светлый' },
    defaultMode: 'light',
    look: 'minimal',
    layout: 'tabbar',
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

/**
 * «Минимал» с боковой панелью — второй вид «Минимала», как «Простор» у «Стекла»: на компьютере панель слева, на
 * телефоне разделы в меню. Палитра, шрифты и элементы — те же.
 */
export const MINIMAL_PANEL: Theme = {
    ...MINIMAL,
    id: 'minimal-panel',
    title: 'Минимал · панель',
    about: 'Тонкие линии и светлые карточки; боковая панель — для компьютера, на телефоне разделы в меню',
    layout: 'sidebar',
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
    return `data-id="${theme.id.replace(/[^a-z0-9-]/g, '')}" data-look="${(theme.look ?? 'glass').replace(/[^a-z]/g, '')}"${mode}`;
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

/** Материалы, которые знает пульт, — и тема, у которой материал взят: её палитра под палитрой пакета. */
const LOOKS: Record<string, Theme> = { glass: GLASS, minimal: MINIMAL };
const isMode = (value: unknown): value is Mode => value === 'dark' || value === 'light';

/**
 * Тема из пакета (kind: theme или плагин с contributes.themes) — чужие данные, поэтому каждое поле проверяется, а чего
 * нет, берётся у темы её материала («Стекло» или «Минимал»): палитра пакета ложится поверх её палитры, как
 * `{ ...GLASS.palette.dark, bg: '#…' }`. Раскладка — только известная оболочка; своя раскладка — это код, плагин
 * пульта (THEMES.md).
 */
export function normalizeTheme(raw: unknown): Theme | null {
    if (!isRecord(raw)) return null;
    const id = typeof raw.id === 'string' && /^[a-z0-9-]{1,40}$/.test(raw.id) ? raw.id : null;
    const title = typeof raw.title === 'string' ? raw.title.trim().slice(0, 40) : '';
    if (!id || !title || !isRecord(raw.palette)) return null;
    const look = typeof raw.look === 'string' && raw.look in LOOKS ? raw.look : 'glass';
    const base = LOOKS[look];
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
        look,
        palette: {
            dark: { ...base.palette.dark, ...paletteOf(raw.palette.dark) },
            light: { ...base.palette.light, ...paletteOf(raw.palette.light) },
        },
        layout: raw.layout === 'sidebar' ? 'sidebar' : 'tabbar',
        density: clamp(raw.density, 0.8, 1.5, 1),
        font: typeof raw.font === 'string' && raw.font.trim() ? raw.font.slice(0, 200) : base.font,
        radius: {
            panel: clamp(radius.panel, 0, 48, base.radius.panel),
            large: clamp(radius.large, 0, 56, base.radius.large),
            bar: clamp(radius.bar, 0, 48, base.radius.bar),
        },
    };
}
