// MagicPlace на столе и в доке (docs/plans/magicplace.md): маленький виджет — знак, сколько обновлений, что новое или
// рекомендуем, нажатие открывает окно; у ячейки дока — число обновлений. Оба читают общий снимок каталога (state.ts),
// а не каталог при каждой перемене на шлюзе.

import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { isInstalled } from '../screens/catalog';
import { goesHere, isNew, recommendedHere, shownAlone } from './filters';
import { MagicPlaceMark } from './icon';
import { useStoreView } from './state';

/** Число обновлений у ячейки дока; нет — ничего. */
export function UpdatesBadge() {
    const view = useStoreView();
    const updates = (view?.packages ?? []).filter((entry) => entry.state === 'update').length;
    if (updates === 0) return null;
    return (
        <span class="mp-badge" role="img" aria-label={tr.magicplace.widgetUpdates(updates)}>
            {updates > 9 ? '9+' : updates}
        </span>
    );
}

/** Виджет стола: знак MagicPlace и одна строка — обновления, новое, рекомендуем или что в нём есть. */
export function MagicPlaceWidget() {
    const panel = usePanel();
    const view = useStoreView();
    const packages = view?.packages ?? [];
    const updates = packages.filter((entry) => entry.state === 'update').length;
    const offered = packages.filter((entry) => !isInstalled(entry) && goesHere(entry) && shownAlone(entry));
    const fresh = offered.find((entry) => isNew(entry));
    const recommended = offered.find(recommendedHere);
    const line = updates > 0
        ? tr.magicplace.widgetUpdates(updates)
        : fresh
          ? tr.magicplace.widgetNew(fresh.title)
          : recommended
            ? tr.magicplace.widgetRecommended(recommended.title)
            : tr.magicplace.widgetIdle;
    return (
        <button class="mp-widget" aria-label={tr.magicplace.widgetOpen} onClick={() => panel.go('magicplace')}>
            <MagicPlaceMark size={40} />
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="w-strong">{tr.magicplace.title}</span>
                <span class={`w-cap${updates > 0 ? ' c-accent-text' : ''}`}>{line}</span>
            </span>
        </button>
    );
}
