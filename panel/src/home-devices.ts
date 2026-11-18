// Устройства дома (docs/plans/home-and-devices.md, этап 1): одно чтение `GET /api/v1/home/devices` у центра на всю
// страницу — верхняя строка дома, «Настройки → Дом → Устройства», сцена стола. В окне компьютера-Турбо (`?via=`)
// запрос идёт центру через проход: своего дома у компьютера нет. Без JSX: его берут экраны.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, api, problemOf } from './api';
import type { Health, HomeDevice, HomeDevices } from './api';
import { tr } from './i18n';
import { usePoll } from './poll';
import { usePanel } from './shell/context';
import { VIA } from './via';
import { dotOf, thisDeviceOf } from './home-model';

/** Последний ответ; null — шлюз постарше (404), undefined — ещё не читали. */
let cache: HomeDevices | null | undefined;
let failure: string | null = null;
let readAt = 0;
let reading: Promise<void> | null = null;
let stale = false;
const listeners = new Set<() => void>();

const tell = (): void => {
    for (const listener of [...listeners]) listener();
};

/** Прочитать список сейчас (один запрос, сколько бы ни просили). */
export function readHomeDevices(): Promise<void> {
    if (reading) return reading;
    stale = false;
    const read: Promise<void> = api
        .homeDevices()
        .then(
            (view) => {
                cache = view;
                failure = null;
            },
            (e) => {
                if (e instanceof ApiError && e.status === 404) {
                    cache = null;
                    failure = null;
                } else failure = problemOf(e);
            },
        )
        .finally(() => {
            readAt = Date.now();
            if (reading === read) reading = null;
            tell();
        });
    reading = read;
    return read;
}

/** Новость живого канала (app.tsx): список поменялся — следующее чтение не пропускается как «только что». */
export function noteHomeDevicesNews(name: string): void {
    if (name === 'home.devices.changed' || name === 'hello') stale = true;
}

/** Список дома: читается раз в 30 с и по новостям канала, не чаще раза в 2 с без новостей. */
export function useHomeDevices(): { view?: HomeDevices; older: boolean; error: string | null } {
    const panel = usePanel();
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = (): void => rerender((n) => n + 1);
        listeners.add(update);
        return () => {
            listeners.delete(update);
        };
    }, []);
    usePoll({
        every: 30_000,
        version: panel.version,
        read: () => (!stale && cache !== undefined && Date.now() - readAt < 2_000 ? Promise.resolve() : readHomeDevices()),
    });
    return { view: cache ?? undefined, older: cache === null, error: failure };
}

// Этот компьютер в окне дома (`?via=`): пульт открыт на нём, а /health через проход — центра. Свой /health компьютер
// берёт у себя, мимо прохода, один раз: id его шлюза и колонки, которые он знает.
let ownGateway: string | null | undefined;
let ownHealth: Health | null = null;
/** Пульт открыт на самом шлюзе: окно приложения DuoWave на этом компьютере. */
const LOCAL_HOST = ['127.0.0.1', 'localhost', '[::1]'].includes(location.hostname);
const ownListeners = new Set<() => void>();

function readOwnGateway(): void {
    if (ownGateway !== undefined) return;
    ownGateway = null;
    // Шлюз отвечает в обёртке `{code, message}` (как и на всё остальное): сам /health — в `message`. Без этого окно на
    // Mac не знало свой id и не находило себя в списке дома — у Mac не было «Этот компьютер» (проверка начисто 18.11,
    // п. 34: строка разработчика «own — · via · this —» при совпадающих id).
    void fetch('/health', { headers: { Accept: 'application/json' } })
        .then((response) => (response.ok ? (response.json() as Promise<{ message?: Health } | null>) : null))
        .then((envelope) => {
            const health = envelope?.message ?? null;
            ownHealth = health;
            // Не прочиталось — прочитать снова при следующем показе, а не считать, что своего шлюза нет.
            ownGateway = health?.gateway?.id ?? undefined;
            for (const listener of [...ownListeners]) listener();
        })
        .catch(() => {
            ownGateway = undefined;
        });
}

/** /health этого компьютера (в окне дома — мимо прохода); ещё не прочитан — null. */
export function useOwnHealth(): Health | null {
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = (): void => rerender((n) => n + 1);
        ownListeners.add(update);
        readOwnGateway();
        return () => {
            ownListeners.delete(update);
        };
    }, []);
    return ownHealth;
}

/**
 * Устройство, на котором открыт этот пульт: в окне дома на компьютере — компьютер (по id его шлюза), иначе — центр,
 * если пульт открыт у него. В браузере телефона «этого» устройства нет.
 */
export function useThisDevice(view: HomeDevices | undefined, health: Health | null): HomeDevice | undefined {
    const [, rerender] = useState(0);
    useEffect(() => {
        // Окно дома на компьютере — во всё окно или во фрейме пульта компьютера: «этот» компьютер узнаётся по его
        // /health в обоих (проверка начисто 18.11, п. 29: во фрейме он не читался — у Mac не было «Этот компьютер»).
        if (!VIA && !LOCAL_HOST) return;
        const update = (): void => rerender((n) => n + 1);
        ownListeners.add(update);
        readOwnGateway();
        return () => {
            ownListeners.delete(update);
        };
    }, []);
    return thisDeviceOf(view, ownGatewayId(health), !!VIA, LOCAL_HOST);
}

/**
 * Id шлюза, на котором открыт этот пульт: в окне дома и в окне приложения на этом компьютере — по его /health напрямую,
 * мимо прохода и мимо того, что пульт знает о центре; в браузере — по /health пульта.
 */
function ownGatewayId(health: Health | null): string | null | undefined {
    if (VIA) return ownGateway;
    if (LOCAL_HOST) return ownGateway ?? health?.gateway?.id;
    return health?.gateway?.id;
}

/** Для слоя разработчика: как пульт узнаёт «этот» компьютер — свой id, где открыт, id в списке дома. */
export function useThisDeviceProbe(view: HomeDevices | undefined, health: Health | null): string {
    const device = useThisDevice(view, health);
    const where = VIA ? 'via' : LOCAL_HOST ? 'local' : 'browser';
    const ids = (view?.devices ?? []).map((entry) => `${entry.id}=${entry.gatewayId ?? '—'}${entry.itself ? '*' : ''}`).join(', ');
    return `own ${ownGatewayId(health) ?? '—'} · ${where} · this ${device?.id ?? '—'} · list ${ids}`;
}

/** Центр дома в списке. */
export function centreOf(view: HomeDevices | undefined): HomeDevice | undefined {
    return view?.devices.find((device) => device.id === view.centre);
}

/** Слова состояния устройства: «на связи», «спит», «не на связи»… */
export function deviceStateWords(device: HomeDevice): string {
    const t = tr.devices;
    switch (device.state) {
        case 'online':
            return t.stateOnline;
        case 'reconnecting':
            return t.stateReconnecting;
        case 'asleep':
            return t.stateAsleep;
        case 'offline':
            return t.stateOffline;
        case 'off':
            return t.stateOff;
        default:
            return t.stateUnknown;
    }
}

/** Класс точки связи: зелёная, жёлтая, серая. */
export function deviceDot(device: HomeDevice): string {
    return `dot dot-${dotOf(device)}`;
}

/** Имя устройства для людей: «Колонка «Кухня»», «MacBook»; марка — только в подписи. */
export function deviceTitle(device: HomeDevice): string {
    return device.kind === 'station' ? tr.devices.stationNamed(device.name) : device.name;
}
