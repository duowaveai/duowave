#!/usr/bin/env node
/**
 * Знаки сервисов для пульта. Скачивает закреплённые SVG из Simple Icons и LobeHub Icons, берёт из них только контур,
 * цвет бренда и имя и для каждой темы решает: цвет бренда — или цвет текста (currentColor), если цвет бренда даёт
 * на круглом значке строки меньше 3:1 (WCAG 2.2, 1.4.11 — графика). Пишет src/brand-icons.ts и src/brand-icons.css;
 * те же входы всегда дают те же файлы: версии закреплены, в выводе нет дат.
 *
 *   node panel/scripts/brand-icons.mjs                скачать и записать оба файла
 *   node panel/scripts/brand-icons.mjs --cache DIR    скачанное — в DIR; в следующий раз оттуда, без сети
 *   node panel/scripts/brand-icons.mjs --check        сравнить с файлами на диске и ничего не писать (код 1 — устарели)
 *
 * Без зависимостей: Node 20+ (глобальный fetch). Скачанное — только данные, ничего из него не исполняется: контур,
 * hex и имя проходят строгий шаблон, прежде чем попасть в сгенерированный код.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const PANEL = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const GLASS_CSS = join(PANEL, 'src/ui/glass.css');
const OUT_TS = join(PANEL, 'src/brand-icons.ts');
const OUT_CSS = join(PANEL, 'src/brand-icons.css');

const SIMPLE = { title: 'Simple Icons', pkg: 'simple-icons', version: '16.36.0', license: 'CC0-1.0', home: 'https://simpleicons.org' };
const LOBE = {
    title: 'LobeHub Icons',
    pkg: '@lobehub/icons-static-svg',
    version: '1.102.0',
    license: 'MIT',
    home: 'https://github.com/lobehub/lobe-icons',
};

/** Знак пульта → откуда он: slug в Simple Icons или имя в LobeHub Icons. Порядок — порядок в сгенерированных файлах. */
const MARKS = [
    { id: 'claude', source: SIMPLE, name: 'claude' },
    { id: 'anthropic', source: SIMPLE, name: 'anthropic' },
    // В Simple Icons 16.36.0 знака OpenAI нет (из набора убран): он из LobeHub Icons.
    { id: 'openai', source: LOBE, name: 'openai' },
    { id: 'openrouter', source: SIMPLE, name: 'openrouter' },
    { id: 'ollama', source: SIMPLE, name: 'ollama' },
    { id: 'lmstudio', source: SIMPLE, name: 'lmstudio' },
    { id: 'gemini', source: SIMPLE, name: 'googlegemini' },
    { id: 'deepseek', source: SIMPLE, name: 'deepseek' },
    { id: 'mistral', source: SIMPLE, name: 'mistralai' },
    { id: 'groq', source: LOBE, name: 'groq' },
    { id: 'homeassistant', source: SIMPLE, name: 'homeassistant' },
    { id: 'notion', source: SIMPLE, name: 'notion' },
    { id: 'todoist', source: SIMPLE, name: 'todoist' },
    { id: 'brave', source: SIMPLE, name: 'brave' },
    // Цветной знак Tavily — три стрелки в шести цветах; цвет бренда — синий основной стрелки (скрипт проверяет, что он в файле).
    { id: 'tavily', source: LOBE, name: 'tavily', pick: '#468BFF' },
    { id: 'mcp', source: SIMPLE, name: 'modelcontextprotocol' },
    { id: 'telegram', source: SIMPLE, name: 'telegram' },
    { id: 'livekit', source: SIMPLE, name: 'livekit' },
    { id: 'asterisk', source: SIMPLE, name: 'asterisk' },
    { id: 'huggingface', source: SIMPLE, name: 'huggingface' },
    { id: 'rss', source: SIMPLE, name: 'rss' },
];

/** WCAG 2.2, 1.4.11: графика — не меньше 3:1 к соседнему цвету. */
const MIN_CONTRAST = 3;
const DEFAULT_VIEWBOX = '0 0 24 24';
const THEMES = [
    { id: 'dark', name: 'ночь' },
    { id: 'light', name: 'туман' },
];

const PATH_DATA = /^[MmLlHhVvCcSsQqTtAaZz0-9eE.,+\-\s]+$/;
const TITLE = /^[\p{L}\p{N}][\p{L}\p{N} .&+'’-]{0,59}$/u;

const options = parseArgs(process.argv.slice(2));
const slot = slotBackgrounds();
const simple = brandsBySlug(JSON.parse(await need(SIMPLE, 'data/simple-icons.json')));
const marks = await Promise.all(MARKS.map(readMark));
for (const mark of marks) {
    for (const theme of THEMES) {
        const ratio = mark.hex === null ? null : contrast(rgbOf(mark.hex), slot[theme.id].rgb);
        mark[theme.id] = { ratio, colour: ratio !== null && ratio >= MIN_CONTRAST ? mark.hex : null };
    }
}

const files = [
    [OUT_TS, typescript()],
    [OUT_CSS, stylesheet()],
];
if (options.check) {
    const stale = files.filter(([file, text]) => !existsSync(file) || readFileSync(file, 'utf8') !== text);
    for (const [file] of stale) console.error(`устарел: ${file} — запустите скрипт без --check`);
    if (!stale.length) console.log('brand-icons.ts и brand-icons.css совпадают с источниками');
    process.exit(stale.length ? 1 : 0);
}
for (const [file, text] of files) writeFileSync(file, text);
report();

// ─── Загрузка ────────────────────────────────────────────────────

function parseArgs(argv) {
    const out = { cache: null, check: false };
    for (let i = 0; i < argv.length; i++) {
        if (argv[i] === '--check') out.check = true;
        else if (argv[i] === '--cache' && argv[i + 1]) out.cache = resolve(argv[++i]);
        else fail(`непонятный аргумент: ${argv[i]} (есть --cache DIR и --check)`);
    }
    return out;
}

function fail(message) {
    console.error(`brand-icons: ${message}`);
    process.exit(2);
}

/** Закреплённый файл: из кэша, если он там есть, иначе с CDN (и в кэш). null — такого файла в пакете нет. */
async function load(source, path) {
    const url = `https://cdn.jsdelivr.net/npm/${source.pkg}@${source.version}/${path}`;
    const file = options.cache && join(options.cache, `${source.pkg}@${source.version}`, path);
    if (file && existsSync(file)) return readFileSync(file, 'utf8');
    if (file && existsSync(`${file}.missing`)) return null;
    const response = await fetch(url);
    if (response.status === 404) {
        if (file) save(`${file}.missing`, '');
        return null;
    }
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    const text = await response.text();
    const limit = path.endsWith('.json') ? 16 << 20 : 512 << 10;
    if (text.length > limit) throw new Error(`${url}: подозрительно большой файл (${text.length} байт)`);
    if (file) save(file, text);
    return text;
}

async function need(source, path) {
    const text = await load(source, path);
    if (text === null) throw new Error(`${source.pkg}@${source.version}/${path}: нет в пакете`);
    return text;
}

function save(file, text) {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
}

// ─── Разбор источников ───────────────────────────────────────────

async function readMark(mark) {
    const where = `${mark.source.pkg}@${mark.source.version}/icons/${mark.name}.svg`;
    const shape = readShape(await need(mark.source, `icons/${mark.name}.svg`), where);
    if (mark.source === SIMPLE) {
        const brand = simple.get(mark.name);
        if (!brand) throw new Error(`simple-icons.json: нет записи для «${mark.name}»`);
        const where = `simple-icons.json «${mark.name}»`;
        return { ...mark, ...shape, title: checkedTitle(brand.title, where), hex: hexOf(brand.hex, where) };
    }
    // LobeHub: одноцветный знак — <name>.svg, цвет бренда — из <name>-color.svg, если он есть (у Groq его нет).
    const colour = await load(LOBE, `icons/${mark.name}-color.svg`);
    return { ...mark, ...shape, hex: colour === null ? null : brandColour(colour, where.replace(/\.svg$/, '-color.svg'), mark.pick) };
}

/** Записи Simple Icons по slug: имя и hex. Slug — поле slug или имя, сведённое так же, как это делает сам Simple Icons. */
function brandsBySlug(data) {
    const icons = Array.isArray(data) ? data : data?.icons;
    if (!Array.isArray(icons)) throw new Error('simple-icons.json: неожиданный формат');
    const out = new Map();
    for (const icon of icons) {
        if (typeof icon?.title !== 'string' || typeof icon?.hex !== 'string') continue;
        out.set(typeof icon.slug === 'string' ? icon.slug : titleToSlug(icon.title), { title: icon.title, hex: icon.hex });
    }
    return out;
}

/** titleToSlug из simple-icons/sdk. */
function titleToSlug(title) {
    const replacements = { '+': 'plus', '.': 'dot', '&': 'and', đ: 'd', ħ: 'h', ı: 'i', ĸ: 'k', ŀ: 'l', ł: 'l', ß: 'ss', ŧ: 't', ø: 'o' };
    return title
        .toLowerCase()
        .replace(/[+.&đħıĸŀłßŧø]/g, (char) => replacements[char])
        .normalize('NFD')
        .replace(/[^a-z\d]/g, '');
}

/** Открывающие теги SVG: имя и атрибуты. */
function tags(svg) {
    return [...svg.matchAll(/<([a-zA-Z][\w:-]*)\b([^>]*)>/g)].map(([, name, rest]) => {
        const attributes = {};
        for (const [, key, dq, sq] of rest.matchAll(/([\w:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) attributes[key] = dq ?? sq;
        return { name, attributes };
    });
}

/**
 * Одноцветный знак: только <svg>, <title> и <path> с заливкой текущим цветом. Всё остальное (группы, фигуры,
 * прозрачность, трансформации) останавливает скрипт, а не теряется молча.
 */
function readShape(svg, where) {
    const all = tags(svg);
    const root = all.find((tag) => tag.name === 'svg');
    if (!root) throw new Error(`${where}: нет <svg>`);
    const viewBox = (root.attributes.viewBox ?? DEFAULT_VIEWBOX).trim().replace(/\s+/g, ' ');
    if (!/^-?[\d.]+ -?[\d.]+ [\d.]+ [\d.]+$/.test(viewBox)) throw new Error(`${where}: странный viewBox «${viewBox}»`);
    const paths = [];
    for (const tag of all) {
        if (tag.name === 'svg' || tag.name === 'title') continue;
        if (tag.name !== 'path') throw new Error(`${where}: <${tag.name}> не поддерживается — только <path>`);
        const { d, fill, ...rest } = tag.attributes;
        const extra = Object.keys(rest).filter((key) => key !== 'fill-rule' && key !== 'clip-rule');
        if (extra.length) throw new Error(`${where}: у <path> есть ${extra.join(', ')} — скрипт их не переносит`);
        if (fill !== undefined && fill !== 'currentColor') throw new Error(`${where}: <path fill="${fill}"> — ждали одноцветный знак`);
        if (typeof d !== 'string' || !PATH_DATA.test(d)) throw new Error(`${where}: контур не похож на данные пути`);
        paths.push({ d: d.trim().replace(/\s+/g, ' '), fillRule: tag.attributes['fill-rule'] ?? root.attributes['fill-rule'] ?? 'nonzero' });
    }
    if (!paths.length) throw new Error(`${where}: нет <path>`);
    const rules = new Set(paths.map((path) => path.fillRule));
    if (rules.size > 1 || !['nonzero', 'evenodd'].includes(paths[0].fillRule)) throw new Error(`${where}: разные или странные fill-rule`);
    if (paths.length > 1) throw new Error(`${where}: ${paths.length} контура — проверьте, что их можно слить в один, и разрешите это здесь`);
    const title = svg.match(/<title>([^<]*)<\/title>/)?.[1];
    return {
        title: title === undefined ? undefined : checkedTitle(decodeEntities(title), where),
        path: paths[0].d,
        fillRule: paths[0].fillRule === 'evenodd' ? 'evenodd' : undefined,
        viewBox: viewBox === DEFAULT_VIEWBOX ? undefined : viewBox,
    };
}

/** Цвет бренда из цветного файла LobeHub: его единственная сплошная заливка, а если цветов несколько — выбранный в MARKS. */
function brandColour(svg, where, pick) {
    const colours = new Set();
    for (const tag of tags(svg)) {
        for (const value of [tag.attributes.fill, tag.attributes['stop-color']]) {
            if (value === undefined || value === 'none' || value === 'currentColor' || value.startsWith('url(')) continue;
            colours.add(hexOf(value, where));
        }
    }
    const found = [...colours].join(', ') || 'ни одного';
    if (pick !== undefined) {
        if (!colours.has(pick)) throw new Error(`${where}: выбранного цвета ${pick} в файле нет (есть ${found})`);
        return pick;
    }
    if (colours.size !== 1) throw new Error(`${where}: ждали один цвет бренда, нашли ${found} — выберите его полем pick в MARKS`);
    return [...colours][0];
}

function decodeEntities(text) {
    const named = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" };
    return text.replace(/&(#\d+|#x[\da-f]+|\w+);/gi, (whole, code) =>
        code[0] !== '#' ? (named[code] ?? whole) : String.fromCodePoint(code[1] === 'x' || code[1] === 'X' ? parseInt(code.slice(2), 16) : +code.slice(1)),
    );
}

function checkedTitle(title, where) {
    const clean = title.trim();
    if (!TITLE.test(clean)) throw new Error(`${where}: странное имя «${clean}»`);
    return clean;
}

function hexOf(value, where) {
    const match = value.trim().match(/^#?([\da-f]{6})$/i);
    if (!match) throw new Error(`${where}: цвет «${value}» — ждали #RRGGBB`);
    return `#${match[1].toUpperCase()}`;
}

// ─── Цвет и контраст ─────────────────────────────────────────────

/**
 * Фон значка строки, каким он выходит на экране: --icon-bg поверх --glass-bg поверх --bg (значок 40/38px на
 * стеклянной карточке, DESIGN.md: Route Row). Сияние под стеклом не учитываем: оно декоративное и разное по экрану.
 */
function slotBackgrounds() {
    const css = readFileSync(GLASS_CSS, 'utf8');
    const out = {};
    for (const theme of THEMES) {
        const selector = `html[data-theme='${theme.id}']`;
        const at = css.indexOf(`${selector} {`);
        if (at < 0) throw new Error(`ui/glass.css: нет блока ${selector}`);
        const body = css.slice(css.indexOf('{', at) + 1, css.indexOf('}', at));
        const token = (name) => {
            const match = body.match(new RegExp(`(?:^|[\\s;])${name}\\s*:\\s*([^;]+);`));
            if (!match) throw new Error(`ui/glass.css: в ${selector} нет ${name}`);
            return cssColour(match[1].trim(), `${selector} ${name}`);
        };
        const page = token('--bg');
        if (page.alpha !== 1) throw new Error(`ui/glass.css: --bg в ${selector} должен быть плотным`);
        const rgb = over(token('--icon-bg'), over(token('--glass-bg'), page.rgb)).map(Math.round);
        out[theme.id] = { rgb, hex: hexOfRgb(rgb) };
    }
    return out;
}

function cssColour(value, where) {
    const hex = value.match(/^#([\da-f]{6})$/i);
    if (hex) return { rgb: rgbOf(value), alpha: 1 };
    const rgba = value.match(/^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/i);
    if (rgba) return { rgb: [+rgba[1], +rgba[2], +rgba[3]], alpha: rgba[4] === undefined ? 1 : +rgba[4] };
    throw new Error(`${where}: цвет «${value}» — ждали #RRGGBB или rgba()`);
}

/** Полупрозрачный цвет поверх плотного: смешение в sRGB, как у браузера. */
function over(top, bottom) {
    return top.rgb.map((channel, i) => channel * top.alpha + bottom[i] * (1 - top.alpha));
}

function rgbOf(hex) {
    return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
}

function hexOfRgb(rgb) {
    return `#${rgb.map((channel) => channel.toString(16).padStart(2, '0')).join('').toUpperCase()}`;
}

/** Относительная яркость и контраст по WCAG 2. */
function luminance(rgb) {
    const [r, g, b] = rgb.map((channel) => {
        const c = channel / 255;
        return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a, b) {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
}

/** Отношение для людей: вниз до сотых, чтобы 2.996 не выглядело как проходные 3.00. */
function ratioText(ratio) {
    return `${(Math.floor(ratio * 100) / 100).toFixed(2)}:1`;
}

// ─── Вывод ───────────────────────────────────────────────────────

function quote(text) {
    return `'${text.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
}

function verdict(mark) {
    if (mark.hex === null) return 'цвета бренда в источнике нет — цвет текста в обеих темах';
    const parts = THEMES.map((theme) => `«${theme.name}» ${ratioText(mark[theme.id].ratio)}${mark[theme.id].colour ? '' : ' → цвет текста'}`);
    return `бренд ${mark.hex}: ${parts.join(', ')}`;
}

function typescript() {
    const sources = `npm ${SIMPLE.pkg}@${SIMPLE.version}`;
    const lobe = `npm ${LOBE.pkg}@${LOBE.version}`;
    const lobeMarks = marks.filter((mark) => mark.source === LOBE).map((mark) => mark.title ?? mark.id);
    const out = [
        '// Сгенерировано скриптом panel/scripts/brand-icons.mjs — руками не править: поправьте скрипт и запустите его снова.',
        '//',
        '// Источники, версии закреплены:',
        `//   ${SIMPLE.title} ${SIMPLE.version} — ${sources}, ${SIMPLE.license}, ${SIMPLE.home}: контуры и цвета брендов.`,
        `//   ${LOBE.title} — ${lobe}, ${LOBE.license}, ${LOBE.home}: ${lobeMarks.join(', ')}.`,
        '// Лицензии и уведомления — panel/THIRD_PARTY.md. Логотипы — знаки их владельцев; пульт показывает их только чтобы',
        '// узнать сервис. Цвет знака не означает исполнителя (DESIGN.md, The Brand-Mark Rule).',
        '//',
        '// dark / light — цвет бренда на этой теме или null: знак берёт цвет текста (currentColor), потому что цвет бренда',
        `// даёт на значке строки меньше ${MIN_CONTRAST}:1 (WCAG 2.2, 1.4.11 — графика). Фон значка — каким он выходит на экране:`,
        '// --icon-bg поверх --glass-bg поверх --bg из ui/glass.css (значок строки на стеклянной карточке), без сияния — оно',
        `// декоративное и разное по экрану: «ночь» ${slot.dark.hex}, «туман» ${slot.light.hex}. Тему переключает brand-icons.css`,
        '// (переменные --brand-<id>), без перерисовки.',
        '',
        'export type BrandId =',
        ...marks.map((mark, i) => `    | ${quote(mark.id)}${i === marks.length - 1 ? ';' : ''}`),
        '',
        'export interface Brand {',
        '    /** Имя сервиса, как его пишет источник. */',
        '    title: string;',
        '    /** Контур знака: одна заливка. */',
        '    path: string;',
        "    fillRule?: 'evenodd';",
        `    /** Если не ${DEFAULT_VIEWBOX}. */`,
        '    viewBox?: string;',
        '    /** Цвет бренда на тёмной теме или null — цвет текста. */',
        '    dark: string | null;',
        '    /** Цвет бренда на светлой теме или null — цвет текста. */',
        '    light: string | null;',
        '}',
        '',
        'export const BRANDS: Record<BrandId, Brand> = {',
    ];
    for (const mark of marks) {
        out.push(`    // ${verdict(mark)}`);
        out.push(`    ${mark.id}: {`);
        out.push(`        title: ${quote(mark.title ?? mark.id)},`);
        for (const theme of THEMES) out.push(`        ${theme.id}: ${mark[theme.id].colour ? quote(mark[theme.id].colour) : 'null'},`);
        if (mark.fillRule) out.push(`        fillRule: ${quote(mark.fillRule)},`);
        if (mark.viewBox) out.push(`        viewBox: ${quote(mark.viewBox)},`);
        out.push(`        path: ${quote(mark.path)},`);
        out.push('    },');
    }
    out.push('};', '');
    return out.join('\n');
}

function stylesheet() {
    const block = (theme) => marks.map((mark) => `    --brand-${mark.id}: ${mark[theme].colour ?? 'currentColor'};`);
    return [
        '/* Сгенерировано скриптом panel/scripts/brand-icons.mjs — руками не править.',
        '   Цвет знака сервиса в каждой теме (данные — brand-icons.ts): цвет бренда или currentColor — цвет текста там, где',
        `   бренд даёт на значке строки меньше ${MIN_CONTRAST}:1. Селекторы — как у токенов тем в ui/glass.css; читает BrandMark (ui/brand.tsx). */`,
        '',
        ':root,',
        "html[data-theme='dark'] {",
        ...block('dark'),
        '}',
        '',
        "html[data-theme='light'] {",
        ...block('light'),
        '}',
        '',
    ].join('\n');
}

function report() {
    const size = (text) => `${Buffer.byteLength(text)} байт`;
    console.log(`значок строки: «ночь» ${slot.dark.hex}, «туман» ${slot.light.hex}; порог ${MIN_CONTRAST}:1`);
    for (const mark of marks) {
        const cells = THEMES.map((theme) => {
            const { ratio, colour } = mark[theme.id];
            return `${(ratio === null ? '—' : ratioText(ratio)).padStart(8)} ${colour ? 'бренд' : 'текст'}`;
        });
        console.log(`  ${mark.id.padEnd(14)} ${(mark.hex ?? '—').padEnd(8)} ${cells.join('   ')}`);
    }
    for (const [file, text] of files) console.log(`записан ${file} (${size(text)})`);
}
