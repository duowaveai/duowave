// Пульт колонки в окне помощника (docs/plans/desk.md, этап 4): `/panel/?via=<колонка>&window=<раздел>` — тот же пульт,
// но его запросы идут через проход компьютера `/station/<колонка>/…` к шлюзу колонки, по ключу пары. Правка
// сохраняется на колонке; колонки нет на связи — шлюз компьютера отдаёт последнюю копию (`x-duowave-copy-at`), а
// правку не пропускает.

const params = (() => {
    try {
        return new URLSearchParams(location.search);
    } catch {
        return new URLSearchParams();
    }
})();

/** Колонка, на которую смотрит этот пульт через проход помощника; null — пульт своего шлюза. */
export const VIA: string | null = params.get('via')?.trim() || null;

/** Раздел, который окно помощника открыло. */
export const WINDOW: string | null = params.get('window')?.trim() || null;

const BASE = VIA ? `/station/${encodeURIComponent(VIA)}` : '';

/** Адрес шлюза: через проход, когда пульт смотрит на колонку. Чужой адрес — как есть. */
export function gw(path: string): string {
    return BASE && path.startsWith('/') && !path.startsWith('//') ? `${BASE}${path}` : path;
}

/** Подпись кнопки сохранения: в окне колонки — «Сохранить на колонке». */
export function saveLabel(label = 'Сохранить'): string {
    return VIA ? `${label} на колонке` : label;
}

// Копия вместо ответа: колонка не на связи, шлюз компьютера показывает, что помнит.
let copyAt: number | null = null;
let liveAt = 0;
const listeners = new Set<() => void>();

/** Ответ пришёл: с отметкой копии — колонки нет, без неё — она на связи. */
export function noteAnswer(header: string | null): void {
    const before = copyAt;
    if (header) {
        const at = Number(header);
        copyAt = Number.isFinite(at) ? Math.min(copyAt ?? at, at) : copyAt;
    } else {
        copyAt = null;
        liveAt = Date.now();
    }
    if (before !== copyAt) for (const listener of [...listeners]) listener();
}

/** Когда снята копия, которую пульт показывает (мс от эпохи); null — колонка отвечает сама. */
export function copyShown(): number | null {
    return copyAt;
}

export function lastLive(): number {
    return liveAt;
}

export function onCopyChange(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}
