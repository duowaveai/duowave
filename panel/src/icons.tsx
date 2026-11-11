/** Инлайновые SVG-иконки из макетов «Стекла» (stroke 2, round). */

type IconProps = { size?: number };

function svg(size: number, children: preact.ComponentChildren, filled = false) {
    return (
        <svg
            aria-hidden="true"
            width={size}
            height={size}
            viewBox="0 0 24 24"
            fill={filled ? 'currentColor' : 'none'}
            stroke={filled ? 'none' : 'currentColor'}
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            {children}
        </svg>
    );
}

export const IconSpeaker = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="6" y="2" width="12" height="20" rx="4" />
        <circle cx="12" cy="14" r="3" />
    </>);

export const IconChat = ({ size = 20 }: IconProps) =>
    svg(size, <path d="M21 11.5a8.4 8.4 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8z" />);

export const IconPlayer = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="6" y="5" width="4" height="14" rx="1.5" />
        <rect x="14" y="5" width="4" height="14" rx="1.5" />
    </>, true);

export const IconMusic = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M9 18V5l12-2v13" />
        <circle cx="6" cy="18" r="3" />
        <circle cx="18" cy="16" r="3" />
    </>);

export const IconTimer = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <circle cx="12" cy="13" r="8" />
        <path d="M12 9v4l2 2" />
        <path d="M10 2h4" />
    </>);

export const IconHome = ({ size = 20 }: IconProps) =>
    svg(size, <path d="M3 10.5 12 3l9 7.5V20a1.5 1.5 0 0 1-1.5 1.5H15v-6H9v6H4.5A1.5 1.5 0 0 1 3 20z" />);

export const IconPhone = ({ size = 20 }: IconProps) =>
    svg(size, <path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.127.96.361 1.903.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.907.339 1.85.573 2.81.7A2 2 0 0 1 22 16.92z" />);

export const IconMoon = ({ size = 20 }: IconProps) =>
    svg(size, <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />);

export const IconSun = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <circle cx="12" cy="12" r="4.5" />
        <line x1="12" y1="1.5" x2="12" y2="3.5" />
        <line x1="12" y1="20.5" x2="12" y2="22.5" />
        <line x1="4.6" y1="4.6" x2="6" y2="6" />
        <line x1="18" y1="18" x2="19.4" y2="19.4" />
        <line x1="1.5" y1="12" x2="3.5" y2="12" />
        <line x1="20.5" y1="12" x2="22.5" y2="12" />
        <line x1="4.6" y1="19.4" x2="6" y2="18" />
        <line x1="18" y1="6" x2="19.4" y2="4.6" />
    </>);

export const IconUser = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
        <circle cx="12" cy="7" r="4" />
    </>);

export const IconMic = ({ size = 24 }: IconProps) =>
    svg(size, <>
        <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3z" stroke-width="2.2" />
        <path d="M19 10v2a7 7 0 0 1-14 0v-2" stroke-width="2.2" />
        <line x1="12" y1="19" x2="12" y2="22" stroke-width="2.2" />
    </>);

export const IconSend = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <path d="M22 2 11 13" stroke-width="2.2" />
        <path d="M22 2 15 22l-4-9-9-4z" stroke-width="2.2" />
    </>);

export const IconLinks = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <path d="M12 22v-5" />
        <path d="M9 8V2" />
        <path d="M15 8V2" />
        <path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8z" />
    </>);

export const IconVoice = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <line x1="4" y1="10" x2="4" y2="14" />
        <line x1="8" y1="6" x2="8" y2="18" />
        <line x1="12" y1="3" x2="12" y2="21" />
        <line x1="16" y1="6" x2="16" y2="18" />
        <line x1="20" y1="10" x2="20" y2="14" />
    </>);

export const IconSmile = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <circle cx="12" cy="12" r="9" />
        <path d="M8.5 14.5a4.5 4.5 0 0 0 7 0" />
        <line x1="9" y1="9.5" x2="9.01" y2="9.5" />
        <line x1="15" y1="9.5" x2="15.01" y2="9.5" />
    </>);

export const IconJournal = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <path d="M14 2v6h6" />
        <line x1="16" y1="13" x2="8" y2="13" />
        <line x1="16" y1="17" x2="8" y2="17" />
    </>);

export const IconRoutes = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <circle cx="5" cy="6" r="3" />
        <circle cx="19" cy="18" r="3" />
        <path d="M8 6h5a4 4 0 0 1 4 4v5" />
    </>);

/** Мозг дашборда: искра. */
export const IconSparkle = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z" />
        <path d="M19 15.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z" />
    </>);

/** Мышление: стрелка прибора. */
export const IconGauge = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <path d="M4 17a8 8 0 1 1 16 0" />
        <path d="M12 17l4.5-5" />
        <circle cx="12" cy="17" r="1.2" />
    </>);

/** Слух: распознавание речи. */
export const IconEar = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <path d="M6 8.5a6 6 0 0 1 12 0c0 3.2-2.2 4.2-3.2 5.6-.8 1.1-.9 2.6-1.9 3.7a3 3 0 0 1-4.9-1.3" />
        <path d="M9.2 9a2.8 2.8 0 0 1 5.6 0c0 1.4-1 2-1.8 2.6" />
    </>);

/** MCP-серверы: инструменты, связанные в граф. */
export const IconMcp = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <circle cx="6" cy="6" r="2.5" />
        <circle cx="18" cy="6" r="2.5" />
        <circle cx="12" cy="18" r="2.5" />
        <path d="M8.5 6h7M7.3 8.2l3.5 7.6M16.7 8.2l-3.5 7.6" />
    </>);

/** Помощник-агент. */
export const IconRobot = ({ size = 22 }: IconProps) =>
    svg(size, <>
        <rect x="5" y="8" width="14" height="11" rx="3" />
        <path d="M12 8V5" />
        <circle cx="12" cy="4" r="1" />
        <line x1="9.5" y1="13" x2="9.51" y2="13" />
        <line x1="14.5" y1="13" x2="14.51" y2="13" />
    </>);

/** Свой провайдер: ключ. */
export const IconKey = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <circle cx="8.5" cy="15.5" r="5.5" />
        <path d="M12.4 11.6l8.1-8.1" />
        <path d="M19 5l2 2" />
        <path d="M16 8l2 2" />
    </>);

/** Оформление пульта: палитра. */
export const IconPalette = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.93 0 1.65-.75 1.65-1.69 0-.44-.18-.84-.44-1.13-.29-.29-.44-.65-.44-1.12a1.64 1.64 0 0 1 1.67-1.67h2c3.05 0 5.55-2.5 5.55-5.55C21.97 6 17.46 2 12 2z" />
        <circle cx="8.5" cy="7.5" r="0.5" fill="currentColor" stroke="none" />
        <circle cx="6.5" cy="12" r="0.5" fill="currentColor" stroke="none" />
        <circle cx="8" cy="16.5" r="0.5" fill="currentColor" stroke="none" />
        <circle cx="13.5" cy="6" r="0.5" fill="currentColor" stroke="none" />
        <circle cx="17.5" cy="10" r="0.5" fill="currentColor" stroke="none" />
    </>);

/** Раздел «Настройки»: шестерёнка. */
export const IconGear = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
        <circle cx="12" cy="12" r="3" />
    </>);

/** Программа на этом Mac: сервер через stdio. */
export const IconTerminal = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="3" y="4" width="18" height="16" rx="3" />
        <path d="M7.5 9.5l3 2.5-3 2.5" />
        <path d="M13 14.5h3.5" />
    </>);

/** Сервер по адресу. */
export const IconGlobe = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <circle cx="12" cy="12" r="9" />
        <path d="M3 12h18" />
        <path d="M12 3c2.4 2.5 3.6 5.5 3.6 9s-1.2 6.5-3.6 9c-2.4-2.5-3.6-5.5-3.6-9s1.2-6.5 3.6-9z" />
    </>);

/** Вставить JSON. */
export const IconBraces = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M8.5 4H8a2 2 0 0 0-2 2v3.5a2.5 2.5 0 0 1-2.5 2.5 2.5 2.5 0 0 1 2.5 2.5V18a2 2 0 0 0 2 2h.5" />
        <path d="M15.5 4h.5a2 2 0 0 1 2 2v3.5a2.5 2.5 0 0 0 2.5 2.5 2.5 2.5 0 0 0-2.5 2.5V18a2 2 0 0 1-2 2h-.5" />
    </>);

/** Чтение веб-страниц (fetch): окно браузера с текстом. */
export const IconPage = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="3" y="4" width="18" height="16" rx="3" />
        <path d="M3 9.5h18" />
        <line x1="6.5" y1="6.75" x2="6.51" y2="6.75" />
        <line x1="9.5" y1="6.75" x2="9.51" y2="6.75" />
        <path d="M7 13h10" />
        <path d="M7 16.5h6" />
    </>);

export const IconCheck = ({ size = 14 }: IconProps) => svg(size, <path d="M5 12.5l4.2 4.2L19 7" stroke-width="3" />);

export const IconChevronDown = ({ size = 16 }: IconProps) => svg(size, <path d="M6 9l6 6 6-6" stroke-width="2.4" />);

export const IconPlus = ({ size = 18 }: IconProps) =>
    svg(size, <>
        <line x1="12" y1="5" x2="12" y2="19" stroke-width="2.4" />
        <line x1="5" y1="12" x2="19" y2="12" stroke-width="2.4" />
    </>);

/** Пакеты и каталог: коробка. */
export const IconPackage = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M21 8l-9-5-9 5 9 5 9-5z" />
        <path d="M3 8v8l9 5 9-5V8" />
        <line x1="12" y1="13" x2="12" y2="21" />
    </>);

/** Разделы: открыть боковую панель на узком экране. */
export const IconMenu = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <line x1="4" y1="7" x2="20" y2="7" />
        <line x1="4" y1="12" x2="20" y2="12" />
        <line x1="4" y1="17" x2="14" y2="17" />
    </>);

export function capabilityIconComponent(icon: string): preact.FunctionComponent<IconProps> {
    const map: Record<string, preact.FunctionComponent<IconProps>> = {
        chat: IconChat,
        speaker: IconSpeaker,
        player: IconPlayer,
        music: IconMusic,
        timer: IconTimer,
        home: IconHome,
        phone: IconPhone,
        moon: IconMoon,
        user: IconUser,
    };
    return map[icon] ?? IconChat;
}

/** Закрыть окно или панель. */
export const IconClose = ({ size = 20 }: IconProps) => svg(size, <path d="M6 6l12 12M18 6 6 18" />);

/** «…» у виджета: его меню. */
export const IconDots = ({ size = 20 }: IconProps) => svg(size, <path d="M5 12h.01M12 12h.01M19 12h.01" />);

export const IconMinus = ({ size = 20 }: IconProps) => svg(size, <path d="M5 12h14" />);

/** Компьютер — помощник. */
export const IconLaptop = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="4" y="5" width="16" height="11" rx="2" />
        <path d="M2 19h20" />
    </>);

/** Список: покупки, вопросы. */
export const IconList = ({ size = 20 }: IconProps) => svg(size, <path d="M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01" />);

/** Монеты — курсы валют. */
export const IconCoins = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <ellipse cx="9" cy="7" rx="6" ry="3" />
        <path d="M3 7v5c0 1.7 2.7 3 6 3s6-1.3 6-3V7" />
        <path d="M9 18c0 1.7 2.7 3 6 3s6-1.3 6-3v-5c0-1.4-1.8-2.5-4.3-2.9" />
    </>);

/** Щит — присмотр за колонкой: копии, страховка. */
export const IconShield = ({ size = 20 }: IconProps) => svg(size, <path d="M12 3 4 6v6c0 5 3.4 8.3 8 9 4.6-.7 8-4 8-9V6z" />);

/** Мастерская — код. */
export const IconCode = ({ size = 20 }: IconProps) => svg(size, <path d="m8 7-5 5 5 5M16 7l5 5-5 5M14 4l-4 16" />);

export const IconPlay = ({ size = 20 }: IconProps) => svg(size, <path d="M7 5v14l12-7z" />, true);

export const IconPause = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="6" y="5" width="4" height="14" rx="1.2" />
        <rect x="14" y="5" width="4" height="14" rx="1.2" />
    </>, true);

export const IconPrev = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M19 6 10 12l9 6z" fill="currentColor" />
        <path d="M6 6v12" />
    </>);

export const IconNext = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M5 6l9 6-9 6z" fill="currentColor" />
        <path d="M18 6v12" />
    </>);

/** Сердце — «нравится». */
export const IconHeart = ({ size = 20 }: IconProps) => svg(size, <path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z" />);

/** Погода: солнце с лучами. */
export const IconWeather = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <circle cx="12" cy="12" r="4" />
        <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
    </>);

/** Замок — стол закреплён. */
export const IconLock = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="4" y="11" width="16" height="10" rx="2" />
        <path d="M8 11V7a4 4 0 0 1 8 0v4" />
    </>);

/** Открытый замок — стол можно двигать. */
export const IconUnlock = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="4" y="11" width="16" height="10" rx="2" />
        <path d="M8 11V7a4 4 0 0 1 7.75-1.4" />
    </>);

/** Сетка — расставить виджеты. */
export const IconGrid = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <rect x="4" y="4" width="7" height="7" rx="1.5" />
        <rect x="13" y="4" width="7" height="7" rx="1.5" />
        <rect x="4" y="13" width="7" height="7" rx="1.5" />
        <rect x="13" y="13" width="7" height="7" rx="1.5" />
    </>);

/** Поиск по пульту (⌘K): лупа. */
export const IconSearch = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <circle cx="11" cy="11" r="7" />
        <line x1="20" y1="20" x2="16.2" y2="16.2" />
    </>);

/** Колонка в списке «Настроек» и «Вернуть колонку Яндексу»: стрелка назад по кругу. */
export const IconUndo = ({ size = 20 }: IconProps) =>
    svg(size, <>
        <path d="M4 10h11a5 5 0 0 1 0 10h-3" />
        <polyline points="8 6 4 10 8 14" />
    </>);

/** Стрелка «›» в строках списка: слой «подробнее». */
export const IconChevronRight = ({ size = 20 }: IconProps) =>
    svg(size, <polyline points="9 6 15 12 9 18" />);

/** «Назад» в странице «Настроек» на телефоне. */
export const IconChevronLeft = ({ size = 20 }: IconProps) =>
    svg(size, <polyline points="15 6 9 12 15 18" />);
