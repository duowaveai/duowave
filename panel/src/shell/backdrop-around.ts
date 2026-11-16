// Фон темы в странице компьютера внутри пульта колонки (`?own=`, via.ts): рисовать ли свой (backdrop.tsx). Отдельно от
// страницы, без DOM, — его проверяет scripts/panel-check.ts.

/** Что пульт вокруг фрейма уже нарисовал: атрибуты его <html>, как их ставит applyTheme (shell/theme.ts). */
export interface AroundLook {
    /** 'on' — за его столом фон темы. */
    backdrop?: string;
    themeId?: string;
    /** Режим на его странице: 'light' или 'dark'. */
    theme?: string;
}

/**
 * Фон этой темы в этом режиме уже рисует пульт вокруг: страница во фрейме прозрачна, и свой фон стал бы вторым слоем
 * сцен поверх его. Пульт вокруг рисует другую тему или никакой — темы, что есть только у компьютера, примерка черновика
 * в мастерской (пульт колонки её не знает и стоит в «Минимале») — фон рисует сама страница, как без фрейма.
 */
export function paintedAround(around: AroundLook | null, themeId: string, mode: string): boolean {
    return !!around && around.backdrop === 'on' && around.themeId === themeId && around.theme === mode;
}
