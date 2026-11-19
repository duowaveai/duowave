// Строка дома (docs/plans/home-and-devices.md, этап 1; холст 17.11, доска «Главная»): вверху пульта — «Дом» и его
// устройства капсулами: имя, метка роли («Центр», «Турбо»), точка связи. Вкладок устройств больше нет: пульт один, откуда
// ни открой, — а нажатие на капсулу ведёт на страницу устройства («Настройки → Дом → Устройства»). Светлая капсула —
// устройство, чья страница открыта: строка и страница выбирают одно и то же в обе стороны (владелец, 19.11: на странице
// «Гостиная», а светлым оставался Mac). Страница устройства не открыта — светлое то, на котором открыт пульт. На
// телефоне — только капсулы: имя и точка.

import { FunctionComponent } from 'preact';

import type { HomeDevice } from '../api';
import { deviceDot, deviceStateWords, useHomeDevices, useThisDevice } from '../home-devices';
import { chosenDevice } from '../home-model';
import { tr } from '../i18n';
import { IconCloud, IconLaptop, IconServer, IconSpeaker } from '../icons';
import { usePanel } from './context';

type Icon = FunctionComponent<{ size?: number }>;

const ICONS: Record<HomeDevice['kind'], Icon> = { station: IconSpeaker, computer: IconLaptop, server: IconServer, cloud: IconCloud };

/** Открыть страницу устройства. */
export function openDevice(go: (section: string, tab?: string, item?: string) => void, device: HomeDevice): void {
    go('settings', 'devices', device.id);
}

export function HomeBar({ short = false }: { short?: boolean }) {
    const panel = usePanel();
    const { view } = useHomeDevices();
    const mine = useThisDevice(view, panel.health);
    const chosen = view ? chosenDevice(panel.link, view.devices.map((device) => device.id), mine?.id) : undefined;
    const name = view?.home.name || tr.devices.homeName;
    if (!view) return <span class={`home-bar${short ? ' is-short' : ''}`}><span class="home-bar-name">{name}</span></span>;
    return (
        <span class={`home-bar${short ? ' is-short' : ''}`}>
            <span class="home-bar-name">{name}</span>
            <span class="home-chips" role="group" aria-label={tr.devices.homeBarLabel}>
                {view.devices.map((device) => {
                    const DeviceIcon = ICONS[device.kind];
                    const centre = device.roles.includes('centre');
                    const turbo = device.roles.includes('turbo');
                    const state = deviceStateWords(device);
                    return (
                        <button
                            key={device.id}
                            type="button"
                            class={`home-chip${chosen === device.id ? ' is-this' : ''}${device.state === 'off' ? ' is-off' : ''}`}
                            aria-current={chosen === device.id ? 'true' : undefined}
                            aria-label={`${tr.devices.open(device.name)} · ${state}`}
                            title={state}
                            onClick={() => openDevice(panel.go, device)}
                        >
                            {short ? null : <DeviceIcon size={14} />}
                            <span class="home-chip-name">{device.name}</span>
                            {centre && <span class="home-role is-centre">{tr.devices.roleCentre}</span>}
                            {turbo && <span class="home-role is-turbo">{tr.devices.roleTurbo}</span>}
                            <span aria-hidden="true" class={deviceDot(device)} />
                        </button>
                    );
                })}
            </span>
        </span>
    );
}
