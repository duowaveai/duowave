import { IconSparkle } from '../icons';

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
            aria-label={critical ? 'Срочное обновление шлюза' : 'Новая версия шлюза'}
            role={critical ? 'alert' : undefined}
        >
            <span class="picker-glyph" aria-hidden="true">
                <IconSparkle size={22} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class={critical ? 'fs-14 lh-20 c-dot-offline' : 'fs-14 lh-20'}>
                    {critical ? `Срочное обновление ${version}` : `Доступна версия ${version}`}
                </b>
                <span class="t-caption">
                    {!critical
                        ? 'что нового — в «Обновлениях»'
                        : auto
                          ? 'исправляет важное — шлюз поставит его сам в течение часа'
                          : 'исправляет важное — поставьте его как можно скорее'}
                </span>
            </div>
            <button class="pill pill-own setup-go" onClick={onOpen}>
                Обновить
            </button>
            {!critical && (
                <button class="banner-hide" aria-label="Скрыть до следующей версии" title="Скрыть до следующей версии" onClick={onHide}>
                    ×
                </button>
            )}
        </section>
    );
}
