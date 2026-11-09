// Ссылки внутрь пульта: адрес после # называет раздел, вкладку и то, что в ней открыто, — `#/<раздел>/<вкладка>/<id>?…`.
// Например `#/mind/characters/arthur?install` (так ведёт «Установить в свой шлюз» на сайте каталога): «Мозг» →
// «Персонажи», карточка Артура и окно установки. Переходы в пульте пишут адрес, назад и вперёд браузера — читают его;
// до входа адрес ждёт, после входа пульт открывает то, на что он вёл. Нужны сайту, уведомлениям и разделам плагинов.

/** Место в пульте по адресу. */
export interface Place {
    section: string;
    tab?: string;
    /** Что открыто во вкладке: лист подключения, карточка персонажа, пакет. */
    item?: string;
    /** Параметры после «?»: `install` — без значения (пустая строка). */
    query: Record<string, string>;
}

/** Прежние и короткие имена разделов: ссылка с ними ведёт туда же. */
const SECTION_ALIASES: Record<string, string> = {
    voice: 'models',
    brain: 'mind',
};

/** Прежние имена вкладок: «Характер» стал «Персонажами». */
const TAB_ALIASES: Record<string, Record<string, string>> = {
    mind: { character: 'characters', persona: 'characters' },
};

/**
 * Переехавшее: «Дом» → «Пакеты» и раздел «Мастерская» стали вкладками раздела «Плагины». Пакет по старой ссылке с
 * `?install` — из каталога, без него — из установленного.
 */
function moved(section: string, tab: string | undefined, item: string | undefined, query: Record<string, string>): [string, string | undefined, string | undefined] {
    if (section === 'house' && (tab === 'packages' || tab === 'catalog')) {
        return ['plugins', tab === 'catalog' || 'install' in query ? 'catalog' : 'installed', item];
    }
    if (section === 'workshop') {
        // `#/workshop/drafts/<id>` — черновик; `#/workshop/<id>` тоже понимаем.
        const draft = tab === 'drafts' ? item : (tab ?? item);
        return ['plugins', 'workshop', draft];
    }
    if (section === 'packages') return ['plugins', tab, item];
    return [section, tab, item];
}

const decode = (part: string): string => {
    try {
        return decodeURIComponent(part);
    } catch {
        return part;
    }
};

/** Часть адреса: всё, кроме «/», «?», «#» и пробелов, остаётся читаемым (двоеточие в id персонажа — тоже). */
const encode = (part: string): string => encodeURIComponent(part).replace(/%3A/gi, ':');

/** Место по адресу (location.hash); пустой адрес — null: пульт открывается как обычно. */
export function parsePlace(hash: string): Place | null {
    const raw = hash.replace(/^#\/?/, '');
    if (!raw) return null;
    const at = raw.indexOf('?');
    const path = at >= 0 ? raw.slice(0, at) : raw;
    const search = at >= 0 ? raw.slice(at + 1) : '';
    const parts = path.split('/').filter(Boolean).map(decode);
    if (parts.length === 0) return null;
    const named = SECTION_ALIASES[parts[0]] ?? parts[0];
    const query: Record<string, string> = {};
    for (const [key, value] of new URLSearchParams(search)) query[key] = value;
    const [section, tab, item] = moved(
        named,
        parts[1] ? (TAB_ALIASES[named]?.[parts[1]] ?? parts[1]) : undefined,
        parts.length > 2 ? parts.slice(2).join('/') : undefined,
        query,
    );
    return { section, tab, item, query };
}

/** Адрес места: `#/<раздел>/<вкладка>/<id>?<параметры>`; параметр без значения пишется одним именем. */
export function hashOf(place: Place): string {
    const parts = [place.section];
    if (place.tab) {
        parts.push(place.tab);
        if (place.item) parts.push(place.item);
    }
    const query = Object.entries(place.query)
        .map(([key, value]) => (value ? `${encode(key)}=${encode(value)}` : encode(key)))
        .join('&');
    return `#/${parts.map(encode).join('/')}${query ? `?${query}` : ''}`;
}

/** Записать место в адрес: новый шаг истории (назад вернёт прежнее) или замена текущего. */
export function writePlace(place: Place, replace = false): void {
    const next = hashOf(place);
    if (location.hash === next) return;
    try {
        if (replace) history.replaceState(history.state, '', next);
        else history.pushState(history.state, '', next);
    } catch {
        // песочница без истории: хотя бы адрес
        location.hash = next;
    }
}
