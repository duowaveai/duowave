import { IconSparkle } from '../icons';

/**
 * Строка на главном экране: вышла новая версия шлюза. «Обновить» ведёт в «Дом → Шлюз → Обновления», где человек
 * читает, что нового, и сам соглашается на установку; «×» прячет строку до следующей версии (точка на «Дом» остаётся).
 */
export function UpdateBanner({ version, onOpen, onHide }: { version: string; onOpen: () => void; onHide: () => void }) {
    return (
        <section class="card glass setup-banner" aria-label="Новая версия шлюза">
            <span class="picker-glyph" aria-hidden="true">
                <IconSparkle size={22} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class="fs-14 lh-20">Доступна версия {version}</b>
                <span class="t-caption">что нового — в «Обновлениях»</span>
            </div>
            <button class="pill pill-own setup-go" onClick={onOpen}>
                Обновить
            </button>
            <button class="banner-hide" aria-label="Скрыть до следующей версии" title="Скрыть до следующей версии" onClick={onHide}>
                ×
            </button>
        </section>
    );
}
