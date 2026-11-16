// Помощники (docs/plans/home-and-helpers.md, блок 1): что пульт знает о них — список, «Где сейчас», регулятор — одним
// чтением на страницу «Помощники», виджет стола и «Этот компьютер»; слова умений, мест и положений регулятора; текст
// подключения «адрес ключ» для QR и вставки. Без JSX: его берут экраны.

import type { FunctionComponent } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { ApiError, api, problemOf } from './api';
import type { Ability, Away, HelperKind, HelpersView } from './api';
import { tr } from './i18n';
import { IconDownload, IconEar, IconGlobe, IconHome, IconLaptop, IconPackage, IconSend, IconSparkle, IconUndo, IconVoice } from './icons';
import { usePoll } from './poll';
import { usePanel } from './shell/context';
import { byMode } from './words';

type Icon = FunctionComponent<{ size?: number }>;

/** Every ability of contract 1, in the order people see them. */
export const ABILITIES: Ability[] = ['hearing', 'voice', 'model', 'messages', 'apps', 'history', 'catalog'];
/**
 * «Где сейчас» on the page, the rows the gateway sends; the widget shows the first three. A catalog row only in the
 * developer layer, and only when the gateway sends one (block 1's gateway does not).
 */
export const WHERE_ROWS: Ability[] = ['hearing', 'model', 'voice', 'messages', 'apps', 'history'];

export const ABILITY_ICONS: Record<Ability, Icon> = {
    hearing: IconEar,
    voice: IconVoice,
    model: IconSparkle,
    messages: IconSend,
    apps: IconPackage,
    history: IconUndo,
    catalog: IconDownload,
};

/**
 * An ability's name; where it names a role of a question (who gives it now, what replaces it) — by the verb, as the
 * scene says it (owner, 16.11): «Слышит · Думает · Говорит».
 */
export function abilityTitle(id: Ability, where = false): string {
    const t = tr.helpers;
    if (where && id === 'model') return t.abilityBrain;
    if (where && id === 'hearing') return t.abilityHears;
    if (where && id === 'voice') return t.abilitySpeaks;
    return { hearing: t.abilityHearing, voice: t.abilityVoice, model: t.abilityModel, messages: t.abilityMessages, apps: t.abilityApps, history: t.abilityHistory, catalog: t.abilityCatalog }[id];
}

export function abilityAbout(id: Ability): string {
    const t = tr.helpers;
    return { hearing: t.aboutHearing, voice: t.aboutVoice, model: t.aboutModel, messages: t.aboutMessages, apps: t.aboutApps, history: t.aboutHistory, catalog: t.aboutCatalog }[id];
}

/** «дома» · «мой сервер» · «облако». */
export function placeTitle(kind: HelperKind): string {
    return { home: tr.helpers.placeHome, own: tr.helpers.placeOwn, cloud: tr.helpers.placeCloud }[kind];
}

export function placeIcon(helper: { kind: HelperKind; builtin?: boolean }): Icon {
    return helper.builtin ? IconLaptop : helper.kind === 'home' ? IconHome : IconGlobe;
}

export const AWAYS: Away[] = ['home', 'mine', 'any'];

export function awayTitle(away: Away): string {
    return { home: tr.helpers.awayHome, mine: tr.helpers.awayMine, any: tr.helpers.awayAny }[away];
}

/** What a position changes; «Только дома» without Yandex in «Микрофон» mode, where there is none. */
export function awayLine(away: Away): string {
    const t = tr.helpers;
    return { home: byMode(t.awayHomeLine, t.awayHomeLineComputer), mine: t.awayMineLine, any: t.awayAnyLine }[away];
}

/** `<url> <key>` — what the gateway's payload says and the panel's QR shows. */
export function connectionText(url: string, key: string): string {
    return `${url} ${key}`;
}

/**
 * Pasted or scanned text → address and key: the url part is the token matching ^https?://, the key the rest; a space,
 * `#` or a newline may stand between them; an address alone → url; no scheme and no separator → key.
 */
export function parseConnection(text: string): { url?: string; key?: string } {
    const clean = text.trim();
    const found = /^(https?:\/\/[^\s#]+)[\s#]*([\s\S]*)$/i.exec(clean);
    if (!found) return clean && !/[\s#]/.test(clean) ? { key: clean } : {};
    const url = found[1].replace(/\/+$/, '');
    const key = found[2].trim();
    return key ? { url, key } : { url };
}

/** What a connection key looks like: 26 letters and digits in groups (5-5-5-5-6), or an API key `vgw_…`. */
export const looksLikeKey = (text: string): boolean => /^[0-9a-z]{5}(-?[0-9a-z]{5}){3}-?[0-9a-z]{6}$/i.test(text.trim()) || /^vgw_\S+$/.test(text.trim());

// One read for every component: the page and the widget can be on screen together (the desk behind a section window).
/** undefined — not read yet; null — an older gateway (0.11) that has no list of helpers. */
let cache: HelpersView | null | undefined;
let failure: string | null = null;
let readAt = 0;
let reading: Promise<void> | null = null;
/** Grows with every change's answer and every fresh read: a read asked before it answers what is no longer so. */
let generation = 0;
/** News of the helpers came after the last read was asked (noteHelpersNews): the next read is a fresh one. */
let stale = false;
const listeners = new Set<() => void>();

const tell = (): void => {
    for (const listener of [...listeners]) listener();
};

/** A newer view is coming or here: the read on its way is dropped when it answers, and nobody waits for it. */
const outdate = (): void => {
    generation++;
    reading = null;
};

/**
 * Read `/api/v1/helpers` now (one request however many ask); 404 — an older gateway. `fresh` — after a change whose
 * answer is not the list (the gateway's name, a refusal): a request of its own, not the one already on its way.
 */
export function readHelpers(fresh = false): Promise<void> {
    if (fresh) outdate();
    if (reading) return reading;
    stale = false;
    const asked = generation;
    const read: Promise<void> = api
        .helpers()
        .then(
            (view) => {
                if (asked !== generation) return;
                cache = view;
                failure = null;
                readAt = Date.now();
            },
            (e) => {
                if (asked !== generation) return;
                if (e instanceof ApiError && e.status === 404) {
                    cache = null;
                    failure = null;
                } else failure = problemOf(e);
                readAt = Date.now();
            },
        )
        .finally(() => {
            if (reading === read) reading = null;
            tell();
        });
    reading = read;
    return read;
}

/** Every change's answer is the whole view: everyone shows it at once, and a read asked before it does not undo it. */
export function applyHelpers(view: HelpersView): void {
    outdate();
    cache = view;
    failure = null;
    readAt = Date.now();
    tell();
}

/**
 * The live channel's news of the helpers (app.tsx): the list as last read, or as a read on its way will answer, is no
 * longer so — the read the news asks for (panel.version) is a fresh one, never skipped as «read just now».
 * «Проверить сейчас» answers before a linked helper's link is up (the list still says «не на связи»); its
 * `helpers.up` comes within a second: read then, not on the next 15-s round.
 */
export function noteHelpersNews(name: string): void {
    if (name === 'helpers.up' || name === 'helpers.down' || name === 'helpers.changed') stale = true;
}

// «Подключить шлюз» reads the key when it opens and again only when the key changed — a gateway paired (the key is
// single-use) or someone changed it. Each read lets the key in for 15 more minutes (owner's answer 3: for 15 minutes
// after the sheet is opened), so the panel's other news must not read it.
let keyTurn = 0;
const keyListeners = new Set<() => void>();

/** The live channel's news (app.tsx): `helpers.key` — the key was changed; `helpers.clients` `paired` — it was spent. */
export function noteHelperKey(name: string, change?: string): void {
    if (name !== 'helpers.key' && !(name === 'helpers.clients' && change === 'paired')) return;
    keyTurn++;
    for (const listener of [...keyListeners]) listener();
}

/** Grows with every change of this computer's connection key. */
export function useHelperKeyTurn(): number {
    const [turn, setTurn] = useState(keyTurn);
    useEffect(() => {
        const update = (): void => setTurn(keyTurn);
        update();
        keyListeners.add(update);
        return () => {
            keyListeners.delete(update);
        };
    }, []);
    return turn;
}

/**
 * The helpers as last read, re-read every 15 s and on the panel's changes (live `helpers.*` events), not twice in 2 s
 * — unless news of the helpers came since (noteHelpersNews).
 */
export function useHelpers(): { view?: HelpersView; older: boolean; error: string | null } {
    const panel = usePanel();
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = (): void => rerender((n) => n + 1);
        listeners.add(update);
        return () => {
            listeners.delete(update);
        };
    }, []);
    usePoll({
        every: 15_000,
        version: panel.version,
        read: () => (!stale && cache !== undefined && Date.now() - readAt < 2_000 ? Promise.resolve() : readHelpers(stale)),
    });
    return { view: cache ?? undefined, older: cache === null, error: failure };
}
