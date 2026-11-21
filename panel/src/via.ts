// Пульт колонки в окне помощника (docs/plans/desk.md, этап 4): `/panel/?via=<колонка>&window=<раздел>` — тот же пульт,
// но его запросы идут через проход компьютера `/station/<колонка>/…` к шлюзу колонки, по ключу пары. Правка
// сохраняется на колонке; колонки нет на связи — шлюз компьютера отдаёт последнюю копию (`x-duowave-copy-at`), а
// правку не пропускает.

import { tr } from './i18n';

const params = (() => {
    try {
        return new URLSearchParams(location.search);
    } catch {
        return new URLSearchParams();
    }
})();

/** Колонка, на которую смотрит этот пульт через проход помощника; null — пульт своего шлюза. */
export const VIA: string | null = params.get('via')?.trim() || null;

/**
 * Окно дома, чей центр — другой компьютер или сервер, а этот компьютер его Турбо по связи (ключ `link:…`, этап 7 плана
 * home-setup-flow): проход тот же, но центр — не колонка.
 */
export const VIA_LINK: boolean = !!VIA && VIA.startsWith('link:');

/** Раздел, который окно помощника открыло. */
export const WINDOW: string | null = params.get('window')?.trim() || null;

/**
 * Пульт колонки во всё окно, не во фрейме (docs/plans/panel-rebuild.md, этап 4): компьютер помогает колонке, и окно
 * приложения — её пульт. Всё в нём — о колонке; своё у компьютера — страница «Этот компьютер» (`?own=`).
 */
export const TOP_VIA: boolean = (() => {
    if (!VIA) return false;
    try {
        return window.parent === window;
    } catch {
        return false;
    }
})();

/**
 * Страница компьютера внутри пульта колонки: `?own=helper` — «Этот компьютер», `?own=workshop` — его мастерская.
 * Её запросы идут к шлюзу компьютера напрямую, без прохода.
 */
export const OWN_PAGE: string | null = VIA ? null : params.get('own')?.trim() || null;

/** `?local` — остаться в пульте компьютера, не уходить в пульт колонки (разработчику и когда колонка не пускает). */
export const LOCAL: boolean = params.has('local');

/**
 * `?onboarding` — открыть «Первые шаги» сразу: так их просит окно колонки у помощника («Настройки» → «Пройти первые
 * шаги»). Шаги идут в пульте компьютера — он ведёт колонку; пока они открыты, пульт не уходит в окно колонки (app.tsx).
 */
export const ONBOARDING: boolean = !VIA && params.has('onboarding');

const BASE = VIA ? `/station/${encodeURIComponent(VIA)}` : '';

/** Адрес шлюза: через проход, когда пульт смотрит на колонку. Чужой адрес — как есть. */
export function gw(path: string): string {
    return BASE && path.startsWith('/') && !path.startsWith('//') ? `${BASE}${path}` : path;
}

/** Подпись кнопки сохранения: в окне колонки — «Сохранить на колонке»; в окне центра-компьютера — как есть. */
export function saveLabel(label = tr.core.saveLabel): string {
    return VIA && !VIA_LINK ? tr.core.saveOnStation(label) : label;
}

// Копия вместо ответа: колонка не на связи, шлюз компьютера показывает, что помнит.
let copyAt: number | null = null;
// С какого времени колонка не в сети — как её увидел шлюз компьютера (`x-duowave-away-since`, тестер 22.11: Wi-Fi
// колонки пропадал по нескольку раз в день).
let awaySince: number | null = null;
let liveAt = 0;
const listeners = new Set<() => void>();

/** Ответ пришёл: с отметкой копии — колонки нет (и, если шлюз знает, с какого времени), без неё — она на связи. */
export function noteAnswer(header: string | null, away: string | null = null): void {
    const before = [copyAt, awaySince];
    if (header) {
        const at = Number(header);
        copyAt = Number.isFinite(at) ? Math.min(copyAt ?? at, at) : copyAt;
        const since = away ? Number(away) : NaN;
        if (Number.isFinite(since)) awaySince = since;
    } else {
        copyAt = null;
        awaySince = null;
        liveAt = Date.now();
    }
    if (before[0] !== copyAt || before[1] !== awaySince) for (const listener of [...listeners]) listener();
}

/** Когда снята копия, которую пульт показывает (мс от эпохи); null — колонка отвечает сама. */
export function copyShown(): number | null {
    return copyAt;
}

/** С какого времени колонка не в сети (мс от эпохи); null — шлюз этого не сказал или колонка на связи. */
export function awayShown(): number | null {
    return awaySince;
}

export function lastLive(): number {
    return liveAt;
}

export function onCopyChange(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}
