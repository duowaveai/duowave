import { IconSparkle } from '../icons';
import { tr } from '../i18n';

/**
 * Строка на главном экране: вышла новая версия шлюза. «Обновить» ведёт в «Дом → Шлюз → Обновления», где человек
 * читает, что нового, и сам соглашается на установку; «×» прячет строку до следующей версии (точка на «Дом» остаётся).
 * Срочный выпуск (critical) не прячется и говорит, поставит ли его шлюз сам или это надо сделать человеку.
 */
export function UpdateBanner({
    version,
    critical = false,
    auto = false,
    onOpen,
    onHide,
}: {
    version: string;
    critical?: boolean;
    auto?: boolean;
    onOpen: () => void;
    onHide: () => void;
}) {
    return (
        <section
            class="card glass setup-banner"
            aria-label={critical ? tr.ui.updateUrgentLabel : tr.ui.newGatewayVersion}
            role={critical ? 'alert' : undefined}
        >
            <span class="picker-glyph" aria-hidden="true">
                <IconSparkle size={22} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class={critical ? 'fs-14 lh-20 c-dot-offline' : 'fs-14 lh-20'}>
                    {critical ? tr.ui.updateUrgent(version) : tr.ui.updateAvailable(version)}
                </b>
                <span class="t-caption">
                    {!critical
                        ? tr.ui.updateWhatsNew
                        : auto
                          ? tr.ui.updateAutoSoon
                          : tr.ui.updateInstallSoon}
                </span>
            </div>
            <button class="pill pill-own setup-go" onClick={onOpen}>
                {tr.ui.update}
            </button>
            {!critical && (
                <button class="banner-hide" aria-label={tr.ui.hideUntilNext} title={tr.ui.hideUntilNext} onClick={onHide}>
                    ×
                </button>
            )}
        </section>
    );
}
