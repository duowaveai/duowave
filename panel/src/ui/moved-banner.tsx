import { MovedGateway, Station } from '../api';
import { displayName } from '../screens/stations';
import { drawingOf } from '../stations';

/**
 * Строка на главном экране: шлюз сменил порт (прежний был занят), а колонка всё ещё ходит на старый — её голос никуда
 * не доходит. «Переподключить» открывает мастер с этой колонкой и новым адресом шлюза: тот же путь, что при первом
 * подключении, с согласием и копией настроек колонки.
 */
export function MovedBanner({ moved, name, onReconnect }: { moved: MovedGateway; name?: Station; onReconnect: () => void }) {
    const drawing = drawingOf(name?.look, name?.deviceModel);
    const { Glyph } = drawing;
    return (
        <section class="card glass setup-banner" aria-label="Колонка ждёт шлюз по старому адресу">
            <span class="picker-glyph" aria-hidden="true">
                <Glyph color={drawing.color} size={26} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class="fs-14 lh-20">{name ? displayName(name, drawing) : moved.address}: не слышит шлюз</b>
                <span class="t-caption">
                    ходит на {moved.was}, а шлюз теперь на {moved.now}
                </span>
            </div>
            <button class="pill pill-own setup-go" onClick={onReconnect}>
                Переподключить
            </button>
        </section>
    );
}
