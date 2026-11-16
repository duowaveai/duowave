// Настройки MagicPlace (шестерёнка в окне, docs/plans/magicplace.md): когда каталог читался и «Проверить сейчас»,
// «Свои источники» — свой репозиторий приложений по ссылке (только администратор; оттуда ставятся персонажи и темы, без
// кода, с пометкой «не проверен») — и «Не загружать каталог». Неофициальных лент по умолчанию нет: их добавляют здесь.

import { useState } from 'preact/hooks';

import { api, can, problemOf, sessionStore } from '../api';
import { tr } from '../i18n';
import { CatalogState, OwnSources } from '../screens/catalog';
import { timeAgo } from '../time';
import { Sheet, SkeletonRows, WinRow, WinSection } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { MagicPlaceMark } from './icon';

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

export function StoreSettings({ catalog, onClose }: { catalog: CatalogState; onClose: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const view = catalog.view;

    const setCatalogOn = async (on: boolean) => {
        setBusy(true);
        setError(null);
        try {
            catalog.setView(await api.setPackagesCatalog(on));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <Sheet title={tr.magicplace.settingsTitle} about={tr.magicplace.settingsAbout} icon={<MagicPlaceMark size={40} />} onClose={onClose}>
            {!view && !catalog.absent && !catalog.error && <SkeletonRows rows={2} />}
            {view?.configured && view.enabled && (
                <WinSection group>
                    <div class="win-row">
                        <span class="win-row-s flex-1 min-w-0">{view.fetchedAt ? tr.magicplace.checkedAgo(timeAgo(view.fetchedAt)) : ''}</span>
                        <button class="btn btn-plain btn-sm" disabled={catalog.refreshing} aria-busy={catalog.refreshing || undefined} onClick={() => void catalog.reload(true)}>
                            {catalog.refreshing && <span class="spinner spinner-sm" aria-hidden="true" />}
                            {catalog.refreshing ? tr.magicplace.checking : tr.magicplace.checkNow}
                        </button>
                    </div>
                </WinSection>
            )}
            {isAdmin() && view?.configured && view.enabled && <OwnSources view={view} onView={catalog.setView} />}
            {view?.configured && can('settings.edit') && (
                <WinSection group>
                    <WinRow title={tr.catalog.noCatalogTitle} sub={tr.catalog.noCatalogNote}>
                        <Switch on={!view.enabled} label={tr.catalog.noCatalogTitle} disabled={busy} onChange={(off) => void setCatalogOn(!off)} />
                    </WinRow>
                </WinSection>
            )}
            {error && (
                <p class="win-callout m-0" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
