import { useEffect, useState } from 'preact/hooks';

import { Station, StationGatewayView, UpdateStatus, api, can, problemOf } from '../api';
import { locale, tr } from '../i18n';
import { clock, timeAgo } from '../time';
import { stationUpdatePressable, stationUpdateStep } from '../station-update';
import { checkIsStale, setOwnUpdateStatus, setUpdateStatus } from '../updates';
import { ask } from '../ui/confirm';
import { byMode } from '../words';
import { VIA } from '../via';

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
/**
 * `title` — своё имя карточки: в пульте Турбо это «Этот компьютер», рядом с карточкой колонки. `own` — в окне колонки
 * (`?via=`) обновления самого этого компьютера, у его шлюза мимо прохода (owner 20.11: в окне дома не было видно, что
 * отстал сам Mac, — вкладка Mac показывала версию колонки).
 */
export function Updates({ title, own = false }: { title?: string; own?: boolean } = {}) {
    const [status, setShown] = useState<UpdateStatus | null>(null);
    const [checking, setChecking] = useState(false);
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState(false);
    // Окно колонки (`?via=`): что о ней знает компьютер, который её ведёт, — что сделает «Обновить» и как идёт.
    const viaStation = !!VIA && !own;
    const [computer, setComputer] = useState<StationGatewayView | null>(null);
    const updates = own
        ? { read: api.ownUpdates, check: api.ownCheckUpdates, install: api.ownInstallUpdate }
        : { read: api.updates, check: api.checkUpdates, install: api.installUpdate };
    // Всё, что раздел узнал, видят и точка на «Дом», и строка на главном экране: о шлюзе этого пульта — и, в окне
    // колонки, о самом компьютере (updates.ts — у каждого своё).
    const setStatus = (answer: UpdateStatus | null): void => {
        setShown(answer);
        if (answer) (own ? setOwnUpdateStatus : setUpdateStatus)(answer);
    };
    const readComputer = async (): Promise<void> => {
        if (!viaStation || !VIA) return;
        try {
            setComputer((await api.stationFromComputer(VIA)).station ?? null);
        } catch {
            setComputer(null);
        }
    };
    useEffect(() => {
        let stopped = false;
        const read = () =>
            updates
                .read()
                .then((answer) => {
                    if (stopped) return;
                    setStatus(answer);
                    // Открыли раздел — показываем свежее: шлюз сам смотрит ленту раз в час, а здесь спрашивает
                    // её сейчас, если с прошлой проверки прошло больше 10 минут.
                    if (can('system.update') && !answer.installing && checkIsStale(answer)) {
                        setChecking(true);
                        updates
                            .check()
                            .then((fresh) => !stopped && setStatus(fresh))
                            .catch(() => {})
                            .finally(() => !stopped && setChecking(false));
                    }
                })
                .catch(() => !stopped && setStatus(null));
        void read();
        void readComputer();
        const timer = window.setInterval(() => {
            void read();
            void readComputer();
        }, 60_000);
        return () => {
            stopped = true;
            window.clearInterval(timer);
        };
    }, []);
    useEffect(() => {
        if (!status?.installing) return;
        // Пока ставится, шлюз перезапускается: спрашиваем чаще и ждём, когда ответит новая версия.
        const timer = window.setInterval(() => {
            updates
                .read()
                .then(setStatus)
                .catch(() => undefined);
        }, 3000);
        return () => window.clearInterval(timer);
    }, [status?.installing]);
    // Компьютер обновляет колонку или ждёт себя, чтобы обновить её следом: его вид — чаще, итог виден сразу.
    const computerBusy = !!(computer?.live.updating || computer?.afterComputer);
    useEffect(() => {
        if (!computerBusy) return;
        const timer = window.setInterval(() => void readComputer(), 3000);
        return () => window.clearInterval(timer);
    }, [computerBusy]);
    if (!status) return null;
    const check = async () => {
        setBusy(true);
        setMessage(null);
        try {
            // В окне колонки — и компьютер: что он может дать колонке, решает его лента.
            if (viaStation) await api.ownCheckUpdates().catch(() => undefined);
            const answer = await updates.check();
            setStatus(answer);
            await readComputer();
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
            text: own ? tr.house.installTextOwn : byMode(tr.house.installTextStation, tr.house.installTextDevice),
            ok: tr.house.installOk,
        });
        if (!yes) return;
        setBusy(true);
        setMessage(null);
        try {
            setStatus(await updates.install());
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const offer = status.available;
    // Шлюз колонки обновляет компьютер, который её ведёт (owner 17.11: «обновление есть, а кнопки нет»). В окне колонки
    // через компьютер (`?via=`) кнопка просит компьютер прямо отсюда — и говорит, что он сделает: компьютер ставит колонке
    // только свою версию, выпуск новее него идёт сначала на него (owner 20.11). Без прохода — подсказка шлюза.
    const quiet = !status.canInstall && !status.installing && !status.trial;
    const step = viaStation && quiet ? stationUpdateStep(computer, offer?.version) : null;
    const fromComputer = viaStation && quiet && !!step;
    const installFromComputer = async () => {
        if (!VIA || !stationUpdatePressable(step)) return;
        const yes = await ask({
            title: tr.house.installTitle(step.version),
            text: step.kind === 'computerFirst' ? tr.house.installTextComputerFirst : tr.house.installTextFromComputer,
            ok: tr.house.installOk,
        });
        if (!yes) return;
        setBusy(true);
        setMessage(null);
        try {
            const started = await api.updateStationFromComputer(VIA, offer?.version);
            setMessage({
                ok: true,
                text: started.way === 'computerFirst' ? tr.house.computerFirstStarted(started.version) : tr.house.fromComputerStarted,
            });
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
            await readComputer();
        }
    };
    // «Колонка без компьютера» (owner 20.11): пульт самой колонки, её обновляет компьютер, а его нет рядом.
    const installAlone = async () => {
        if (!offer) return;
        const yes = await ask({ title: tr.house.installAloneTitle(offer.version), text: tr.house.installAloneText, ok: tr.house.installOk });
        if (!yes) return;
        setBusy(true);
        setMessage(null);
        try {
            setStatus(await api.installUpdateAlone());
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const heading = own ? 'updates-own-h' : 'updates-h';
    return (
        <section class="card glass card-list" aria-labelledby={heading}>
            <div class="section-head">
                <h2 id={heading} class="t-title-xs m-0">
                    {title ?? tr.house.updates}
                </h2>
                <span class="t-meta">{tr.house.version(status.current)}</span>
            </div>
            {offer ? (
                <div>
                    {offer.critical && (
                        <p class="t-caption m-0 mb-4 c-dot-offline" role="alert">
                            <b>{tr.house.urgent}</b>
                            {tr.house.urgentText}{' '}
                            {status.autoCritical
                                ? tr.house.urgentAuto
                                : status.managedBy === 'pc'
                                  ? tr.house.urgentByComputer
                                  : tr.house.urgentManual}
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
            {status.hint && offer && !fromComputer && (
                <p class="t-caption m-0 mb-8">
                    {status.hint}
                </p>
            )}
            {/* Что компьютер сделает или почему не может — его словами (gateway/src/placement/watch.rs). */}
            {step?.kind === 'blocked' && <p class="t-caption m-0 mb-8">{step.why}</p>}
            {step?.kind === 'stationAhead' && <p class="t-caption m-0 mb-8">{tr.house.stationAhead(step.station, step.computer)}</p>}
            {step?.kind === 'follows' && (
                <p class="t-caption m-0 mb-8 c-accent-text" role="status">
                    {tr.house.followsComputer(step.version)}
                </p>
            )}
            {viaStation && computer?.live.updating && (
                <p class="t-caption m-0 mb-8 c-accent-text" role="status">
                    {tr.house.fromComputerStarted}
                </p>
            )}
            {viaStation && !computer?.live.updating && computer?.live.updateError && (
                <p class="t-caption m-0 mb-8 c-dot-offline" role="alert">
                    {tr.house.computerFailed(computer.live.updateError)}
                </p>
            )}
            {/* Пока ставится — об этом; не поставилось — почему, словами шлюза. На колонке новая версия сначала
                пробуется из памяти (gateway/src/updates/trial.rs). */}
            {status.trial ? (
                <p class="t-caption m-0 mb-8 c-accent-text" role="status">
                    {status.trial.phase === 'trying'
                        ? tr.house.trialTrying(status.trial.version, clock(status.trial.until))
                        : status.trial.phase === 'waiting'
                          ? tr.house.trialWaiting(status.trial.version)
                          : tr.house.trialStuck(status.trial.version, status.trial.message ?? '')}
                    {status.trial.appsPaused && ` ${tr.house.trialAppsPaused}`}
                </p>
            ) : (
                status.installing && (
                    <p class="t-caption m-0 mb-8 c-accent-text" role="status">
                        {tr.house.installing}
                    </p>
                )
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
                    {stationUpdatePressable(step) && (
                        <button
                            class="pill pill-own pill-btn"
                            disabled={busy || !!computer?.live.updating}
                            onClick={() => void installFromComputer()}
                        >
                            {busy || computer?.live.updating
                                ? tr.house.installingShort
                                : step.kind === 'computerFirst'
                                  ? tr.house.updateBoth(step.version)
                                  : tr.desk.updateTo(step.version)}
                        </button>
                    )}
                    {!VIA && status.canInstallAlone && (
                        <button class="pill pill-off pill-btn" disabled={busy} onClick={() => void installAlone()}>
                            {tr.house.installAlone}
                        </button>
                    )}
                </div>
            )}
        </section>
    );
}
