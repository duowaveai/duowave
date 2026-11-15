// Шапка окна «Приложения» (docs/plans/menu-scheme.md): вкладки «Мои · MagicPlace · Подключения» и «Сделать своё».
// Одна у двух разделов: «Мои» и «Подключения» — вкладки раздела `skills`, MagicPlace — свой раздел `magicplace` со
// своими адресами (`#/magicplace/store/<id>` у сайта и уведомлений), но для человека это одно окно. У MagicPlace —
// цветной знак (владелец, 15.11: «чтобы он отличался и люди туда нажимали») и число обновлений.

import { can, sessionStore } from '../api';
import { tr } from '../i18n';
import { IconPlus } from '../icons';
import { MagicPlaceMark } from '../magicplace/icon';
import { UpdatesBadge } from '../magicplace/widget';
import { usePanel } from '../shell/context';

export type AppsTab = 'mine' | 'magicplace' | 'connectors';

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

export function AppsTabs({ active }: { active: AppsTab }) {
    const panel = usePanel();
    const tabs: Array<{ id: AppsTab; shown: boolean; go: () => void }> = [
        { id: 'mine', shown: true, go: () => panel.go('skills', 'installed') },
        { id: 'magicplace', shown: can('station.view'), go: () => panel.go('magicplace') },
        { id: 'connectors', shown: can('connectors.view'), go: () => panel.go('skills', 'connectors') },
    ];
    return (
        <div class="apps-head">
            <div class="apps-tabs" role="tablist" aria-label={tr.skills.tabsLabel}>
                {tabs
                    .filter((tab) => tab.shown)
                    .map((tab) => (
                        <button key={tab.id} role="tab" aria-selected={tab.id === active} class={`apps-tab${tab.id === active ? ' is-on' : ''}`} onClick={tab.go}>
                            {tab.id === 'magicplace' && <MagicPlaceMark size={18} />}
                            {tab.id === 'mine' ? tr.skills.tabMine : tab.id === 'magicplace' ? tr.magicplace.title : tr.skills.tabConnections}
                            {tab.id === 'magicplace' && <UpdatesBadge />}
                        </button>
                    ))}
            </div>
            {isAdmin() && (
                <button class="apps-make" onClick={() => panel.go('skills', 'workshop')}>
                    <IconPlus size={16} />
                    {tr.skills.makeOwn}
                </button>
            )}
        </div>
    );
}
