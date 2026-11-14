import { MovedGateway, Station } from '../api';
import { displayName } from '../screens/stations';
import { drawingOf } from '../stations';
import { tr } from '../i18n';

/**
 * Строка на главном экране: шлюз сменил порт (прежний был занят), а колонка всё ещё ходит на старый — её голос никуда
 * не доходит. «Переподключить» открывает мастер с этой колонкой и новым адресом шлюза: тот же путь, что при первом
 * подключении, с согласием и копией настроек колонки.
 */
export function MovedBanner({ moved, name, onReconnect }: { moved: MovedGateway; name?: Station; onReconnect: () => void }) {
    const drawing = drawingOf(name?.look, name?.deviceModel);
    const { Glyph } = drawing;
    return (
        <section class="card glass setup-banner" aria-label={tr.ui.movedLabel}>
            <span class="picker-glyph" aria-hidden="true">
                <Glyph color={drawing.color} size={26} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class="fs-14 lh-20">{tr.ui.movedTitle(name ? displayName(name, drawing) : moved.address)}</b>
                <span class="t-caption">
                    {tr.ui.movedAbout(moved.was, moved.now)}
                </span>
            </div>
            <button class="pill pill-own setup-go" onClick={onReconnect}>
                {tr.ui.reconnect}
            </button>
        </section>
    );
}
