// Настройки MagicPlace (шестерёнка в окне, docs/plans/magicplace.md): когда каталог читался и «Проверить сейчас»,
// «Свои источники» — свой репозиторий приложений по ссылке (только администратор; оттуда ставятся персонажи и темы, без
// кода, с пометкой «не проверен») — и «Не загружать каталог». Неофициальных лент по умолчанию нет: их добавляют здесь.

import { useState } from 'preact/hooks';

import { api, can, problemOf, sessionStore } from '../api';
import { tr } from '../i18n';
import { CatalogState, OwnSources } from '../screens/catalog';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
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
            {view?.configured && view.enabled && (
                <div class="flex flex-wrap items-center gap-8">
                    {view.fetchedAt && <span class="t-caption">{tr.magicplace.checkedAgo(timeAgo(view.fetchedAt))}</span>}
                    <Action label={catalog.refreshing ? tr.magicplace.checking : tr.magicplace.checkNow} busy={catalog.refreshing} onClick={() => void catalog.reload(true)} />
                </div>
            )}
            {isAdmin() && view?.configured && view.enabled && <OwnSources view={view} onView={catalog.setView} />}
            {view?.configured && can('settings.edit') && (
                <div class="pkg-switch">
                    <div class="flex-1 min-w-0 flex flex-col gap-2">
                        <span class="fw-700">{tr.catalog.noCatalogTitle}</span>
                        <span class="t-caption">{tr.catalog.noCatalogNote}</span>
                    </div>
                    <Switch on={!view.enabled} label={tr.catalog.noCatalogTitle} disabled={busy} onChange={(off) => void setCatalogOn(!off)} />
                </div>
            )}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
