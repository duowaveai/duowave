// Полоса дома под сценой стола (docs/plans/home-and-devices.md, этап 1; холст 17.11, доска «Главная»): колонки, что
// слышат для центра («Колонка «Спальня» · слышит · копия дома»), плашки тех, кто работает вне дома, — Алиса и облака по
// ключам, — узел «Приложения» и легенда цветов: лайм — центр, фиолет — Турбо, янтарь — облака и Яндекс, серый — спит.
// Сам центр и Турбо — на сцене выше (колонка с орбитой и блок Турбо).

import type { HomeDevice } from '../api';
import { deviceDot, deviceStateWords, useHomeDevices } from '../home-devices';
import { tr } from '../i18n';
import { IconCloud, IconEar, IconPackage, IconSpeaker } from '../icons';
import { usePanel } from '../shell/context';
import { openDevice } from '../shell/home-bar';

export function HomeStrip({ alice, clouds }: { alice: boolean; clouds: boolean }) {
    const panel = usePanel();
    const { view } = useHomeDevices();
    const ears: HomeDevice[] = (view?.devices ?? []).filter((device) => device.kind === 'station' && !device.roles.includes('centre'));
    return (
        <div class="home-strip">
            {ears.map((ear) => (
                <button key={ear.id} type="button" class={`home-ear${ear.state === 'online' ? '' : ' is-off'}`} onClick={() => openDevice(panel.go, ear)}>
                    <IconSpeaker size={16} />
                    <span class="home-ear-text">
                        <b>{tr.devices.stationNamed(ear.name)}</b>
                        <span>
                            <IconEar size={12} /> {tr.devices.hears}
                            {ear.copy ? ` · ${tr.devices.copy}` : ''} · {deviceStateWords(ear)}
                        </span>
                    </span>
                    <span aria-hidden="true" class={deviceDot(ear)} />
                </button>
            ))}
            {alice && (
                <span class="home-plaque is-out">
                    <i aria-hidden="true" />
                    {tr.devices.yandex}
                </span>
            )}
            {clouds && (
                <span class="home-plaque is-out">
                    <IconCloud size={14} />
                    {tr.devices.cloud}
                </span>
            )}
            <button type="button" class="home-plaque is-apps" onClick={() => panel.go('skills')}>
                <IconPackage size={14} />
                {tr.devices.apps}
            </button>
            <span class="home-legend" aria-label={tr.devices.homeBarLabel}>
                <span>
                    <i class="is-centre" aria-hidden="true" />
                    {tr.devices.legendCentre}
                </span>
                <span>
                    <i class="is-turbo" aria-hidden="true" />
                    {tr.devices.legendTurbo}
                </span>
                <span>
                    <i class="is-out" aria-hidden="true" />
                    {tr.devices.legendCloud}
                </span>
                <span>
                    <i class="is-asleep" aria-hidden="true" />
                    {tr.devices.legendAsleep}
                </span>
            </span>
        </div>
    );
}
