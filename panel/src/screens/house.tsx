import { useEffect, useState } from 'preact/hooks';

import { Station, UpdateStatus, api, can, problemOf } from '../api';
import { locale, tr } from '../i18n';
import { timeAgo } from '../time';
import { checkIsStale, setUpdateStatus } from '../updates';
import { ask } from '../ui/confirm';
import { byMode } from '../words';

// Карточки прежнего «Дома»: колонки, которым отвечаем, и обновления DuoWave. Теперь они в «Настройки» → «Колонка» и
// «Обновления и копии» (screens/places.tsx); подключения и MCP — в «Навыках».

const stationPill = (): Record<Station['status'], { cls: string; label: string }> => ({
    approved: { cls: 'pill-own', label: tr.house.pillApproved },
    pending: { cls: 'pill-yandex', label: tr.house.pillPending },
    blocked: { cls: 'pill-off', label: tr.house.pillBlocked },
});

/** Колонки: первая подключилась сама, каждую следующую пускает администратор. */
export function Stations() {
    const [list, setList] = useState<Station[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const mayManage = can('stations.manage');
    const read = () =>
        api
            .stations()
            .then(setList)
            .catch((e) => setError(problemOf(e)));
    useEffect(() => {
        void read();
        const id = setInterval(read, 10_000);
        return () => clearInterval(id);
    }, []);
    const set = async (station: Station, status: Station['status']) => {
        if (
            status === 'blocked' &&
            !(await ask({
                title: tr.house.blockTitle(station.name),
                text: tr.house.blockText,
                ok: tr.house.blockOk,
                danger: true,
            }))
        )
            return;
        setBusy(station.device);
        try {
            await api.updateStation(station.device, { status });
            await read();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    return (
        <section class="card glass card-list card-list-tight" aria-labelledby="stations-h">
            <div class="section-head">
                <h2 id="stations-h" class="t-title-xs m-0">
                    {tr.house.stations}
                </h2>
                {list && (
                    <span class="t-meta">
                        {tr.house.stationsCount(list.length)}
                    </span>
                )}
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {list?.length === 0 && (
                <p class="t-meta m-0 mb-8">
                    {tr.house.noStations}
                </p>
            )}
            {list?.map((station) => (
                <div
                    key={station.device}
                    class="list-row"
                >
                    <div class="flex-1 min-w-160 flex flex-col gap-2">
                        <span class="fw-700">{station.name}</span>
                        <span class="t-caption">
                            {tr.house.lastSeen(station.lastAddress ?? tr.house.addressUnknown, timeAgo(station.lastSeen))}
                        </span>
                    </div>
                    <span class={`pill ${stationPill()[station.status].cls}`}>{stationPill()[station.status].label}</span>
                    {mayManage && station.status !== 'approved' && (
                        <button
                            class="pill pill-own pill-btn"
                            disabled={busy === station.device}
                            onClick={() => void set(station, 'approved')}
                        >
                            {tr.house.allow}
                        </button>
                    )}
                    {mayManage && station.status === 'approved' && (
                        <button
                            class="pill pill-off pill-btn"
                            disabled={busy === station.device}
                            onClick={() => void set(station, 'blocked')}
                        >
                            {tr.house.block}
                        </button>
                    )}
                </div>
            ))}
        </section>
    );
}

/**
 * Обновления шлюза: какая версия стоит и что предлагает лента выпусков. Ставит обновление только тот, у кого есть
 * право (system.update), и только по своему согласию; шлюз в приложении, Docker или Home Assistant обновляет тот,
 * кто им управляет, — тогда здесь подсказка вместо кнопки. Шлюз на Node этого раздела не знает: раздел молчит.
 */
export function Updates() {
    const [status, setShown] = useState<UpdateStatus | null>(null);
    const [checking, setChecking] = useState(false);
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState(false);
    // Всё, что раздел узнал, видят и точка на «Дом», и строка на главном экране.
    const setStatus = (answer: UpdateStatus | null): void => {
        setShown(answer);
        if (answer) setUpdateStatus(answer);
    };
    useEffect(() => {
        let stopped = false;
        const read = () =>
            api
                .updates()
                .then((answer) => {
                    if (stopped) return;
                    setStatus(answer);
                    // Открыли раздел — показываем свежее: шлюз сам смотрит ленту раз в час, а здесь спрашивает
                    // её сейчас, если с прошлой проверки прошло больше 10 минут.
                    if (can('system.update') && !answer.installing && checkIsStale(answer)) {
                        setChecking(true);
                        api.checkUpdates()
                            .then((fresh) => !stopped && setStatus(fresh))
                            .catch(() => {})
                            .finally(() => !stopped && setChecking(false));
                    }
                })
                .catch(() => !stopped && setStatus(null));
        void read();
        const timer = window.setInterval(() => void read(), 60_000);
        return () => {
            stopped = true;
            window.clearInterval(timer);
        };
    }, []);
    useEffect(() => {
        if (!status?.installing) return;
        // Пока ставится, шлюз перезапускается: спрашиваем чаще и ждём, когда ответит новая версия.
        const timer = window.setInterval(() => {
            api.updates()
                .then(setStatus)
                .catch(() => undefined);
        }, 3000);
        return () => window.clearInterval(timer);
    }, [status?.installing]);
    if (!status) return null;
    const check = async () => {
        setBusy(true);
        setMessage(null);
        try {
            const answer = await api.checkUpdates();
            setStatus(answer);
            if (!answer.available)
                setMessage({ ok: true, text: answer.error ? tr.house.feedNoAnswer(answer.error) : tr.house.latestInstalled });
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const install = async () => {
        if (!status.available) return;
        const yes = await ask({
            title: tr.house.installTitle(status.available.version),
            text: byMode(tr.house.installTextStation, tr.house.installTextDevice),
            ok: tr.house.installOk,
        });
        if (!yes) return;
        setBusy(true);
        setMessage(null);
        try {
            setStatus(await api.installUpdate());
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const offer = status.available;
    return (
        <section class="card glass card-list" aria-labelledby="updates-h">
            <div class="section-head">
                <h2 id="updates-h" class="t-title-xs m-0">
                    {tr.house.updates}
                </h2>
                <span class="t-meta">{tr.house.version(status.current)}</span>
            </div>
            {offer ? (
                <div>
                    {offer.critical && (
                        <p class="t-caption m-0 mb-4 c-dot-offline" role="alert">
                            <b>{tr.house.urgent}</b>
                            {tr.house.urgentText} {status.autoCritical ? tr.house.urgentAuto : tr.house.urgentManual}
                        </p>
                    )}
                    <p class="t-body m-0 mb-4">
                        {tr.house.availableVersion}
                        <b>{offer.version}</b>
                        {offer.date ? `, ${new Date(offer.date).toLocaleDateString(locale())}` : ''}
                    </p>
                    {offer.notes && (
                        <p class="t-caption m-0 mb-8 pre-line">
                            {offer.notes}
                        </p>
                    )}
                </div>
            ) : (
                <p class="t-caption m-0 mb-8">
                    {!status.feed
                        ? tr.house.noFeed
                        : checking
                          ? tr.house.checking
                          : status.checkedAt
                          ? tr.house.noNewVersions(timeAgo(status.checkedAt))
                          : tr.house.notChecked}
                </p>
            )}
            {status.hint && offer && (
                <p class="t-caption m-0 mb-8">
                    {status.hint}
                </p>
            )}
            {/* Пока ставится — об этом; не поставилось — почему, словами шлюза. */}
            {status.installing && (
                <p class="t-caption m-0 mb-8 c-accent-text" role="status">
                    {tr.house.installing}
                </p>
            )}
            {status.error && !status.installing && (
                <p class="t-caption m-0 mb-8 c-dot-offline" role="alert">
                    {status.error}
                </p>
            )}
            {message && (
                <p
                    class={`t-caption m-0 mb-8 ${message.ok ? 'c-accent-text' : 'c-dot-offline'}`}
                    role={message.ok ? 'status' : 'alert'}
                >
                    {message.text}
                </p>
            )}
            {can('system.update') && status.feed && (
                <div class="flex gap-8 flex-wrap">
                    <button
                        class="pill pill-off pill-btn"
                        disabled={busy || status.installing}
                        onClick={() => void check()}
                    >
                        {tr.house.check}
                    </button>
                    {status.canInstall && (
                        <button
                            class="pill pill-own pill-btn"
                            disabled={busy || status.installing}
                            onClick={() => void install()}
                        >
                            {status.installing ? tr.house.installingShort : tr.house.update}
                        </button>
                    )}
                </div>
            )}
        </section>
    );
}
