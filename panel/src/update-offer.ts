// Строка о новой версии на главном экране: чья это версия. Без DOM — её проверяет panel-check.
//
// В окне колонки (`?via=`) пульт знает две версии: колонки-центра (через проход) и самого компьютера (мимо прохода).
// Раньше строка смотрела только на колонку, и компьютер, который по 0.17 обновляется первым, оставался без строки;
// а когда колонка не на связи, строки не было вовсе (owner 21.11: в трее 0.17.2 есть, на экране — нет).

/** Предложенная версия: срочная ли она и поставит ли её шлюз сам. */
export interface Offered {
    version: string;
    critical: boolean;
    auto: boolean;
}

/** Что показать строкой: версию и чья она — этого компьютера или шлюза пульта (колонки-центра в её окне). */
export interface BannerOffer extends Offered {
    computer: boolean;
}

/**
 * Компьютер первым: колонка получает новую версию от него, после его обновления. Колонку раньше — только когда её
 * выпуск срочный, а у компьютера нет.
 */
export function bannerOffer(hub: Offered | null, computer: Offered | null): BannerOffer | null {
    if (computer && !(hub?.critical && !computer.critical)) return { ...computer, computer: true };
    return hub ? { ...hub, computer: false } : null;
}
