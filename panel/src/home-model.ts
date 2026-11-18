// Устройства дома без DOM (docs/plans/home-and-devices.md, этап 1): какое из них «это», какие вкладки у страницы
// устройства, какой точкой его рисовать. Берут home-devices.ts и screens/devices.tsx; проверяет panel-check.

import type { CatalogPlace, HomeDevice, HomeDevices } from './api';

/** Вкладки страницы устройства. */
export type DeviceView = 'overview' | 'room' | 'device' | 'computer' | 'homes';

/**
 * Вкладки по ролям: колонка-центр — «Обзор · Комната · Устройство»; колонка, что только слышит, — «Обзор · Комната»;
 * компьютер, на котором открыт пульт, или компьютер-центр — «Обзор · Этот компьютер · Другие дома · Устройство»; чужой
 * компьютер, сервер, облако — «Обзор · Устройство» (своё у них читается у них самих).
 */
export function viewsOf(device: HomeDevice, mine: boolean): DeviceView[] {
    if (device.kind === 'station') return device.roles.includes('centre') ? ['overview', 'room', 'device'] : ['overview', 'room'];
    if (device.kind === 'computer') return mine || device.roles.includes('centre') ? ['overview', 'computer', 'homes', 'device'] : ['overview', 'device'];
    return ['overview', 'device'];
}

/**
 * Устройство, на котором открыт пульт. В окне дома на компьютере (`?via=`) — компьютер, по id его шлюза (`own` — его
 * /health мимо прохода; пока не прочитан — никакое). Пульт, открытый на самом центре (адрес 127.0.0.1 — окно
 * приложения у компьютера-центра), — центр. В браузере телефона или другого компьютера «этого» устройства нет.
 */
export function thisDeviceOf(view: HomeDevices | undefined, own: string | null | undefined, via: boolean, local = false): HomeDevice | undefined {
    if (!view) return undefined;
    if (via) return own ? view.devices.find((device) => device.gatewayId === own) : undefined;
    if (!local) return undefined;
    // Сначала — по id своего шлюза, потом — тот, кто ответил: список отдаёт центр, и его строка («itself») идёт первой.
    // Одним `find` по «или» окно приложения на компьютере-Турбо находило центр, а не себя — у Mac не было «Этот
    // компьютер» (проверка начисто 18.11, пп. 29, 32).
    return (own ? view.devices.find((device) => device.gatewayId === own) : undefined) ?? view.devices.find((device) => device.itself);
}

/** Точка связи: на связи — зелёная; переподключается или ещё не знаем — жёлтая; спит, не на связи, выключен — серая. */
export function dotOf(device: HomeDevice): 'ok' | 'warn' | 'off' {
    if (device.state === 'online') return 'ok';
    if (device.state === 'reconnecting' || device.state === 'unknown') return 'warn';
    return 'off';
}

/**
 * Где в этом доме будет работать пакет с таким местом (docs/plans/home-and-devices.md, этап 5а): плагин — в центре;
 * приложение для компьютера — у компьютера-центра или у первого компьютера-Турбо; приложение для колонки — у
 * колонки-центра; служба в сети — нигде в доме. Нет такого устройства — чего не хватает.
 */
export function placeDevice(place: CatalogPlace | null | undefined, view: HomeDevices | undefined): { device?: HomeDevice; missing?: 'computer' | 'station'; network?: true } {
    if (!place || !view) return {};
    const centre = view.devices.find((device) => device.id === view.centre);
    if (place === 'network') return { network: true };
    if (place === 'gateway') return centre ? { device: centre } : {};
    if (place === 'computer') {
        if (centre?.kind === 'computer') return { device: centre };
        const turbo = view.devices.find((device) => device.kind === 'computer' && device.roles.includes('turbo'));
        return turbo ? { device: turbo } : { missing: 'computer' };
    }
    if (centre?.kind === 'station') return { device: centre };
    const station = view.devices.find((device) => device.kind === 'station');
    return station ? { device: station } : { missing: 'station' };
}
