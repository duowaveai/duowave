// «Ещё колонки — уши одной кнопкой» (docs/plans/home-setup-flow.md, этап 5): колонки, что нашёл поиск этого компьютера и
// которых нет в доме. Без DuoWave — «Сделать ухом»: компьютер ставит DuoWave по SSH сразу ухом центра (перенос «в ухо»),
// код не нужен — нажатие и есть согласие. Со своим DuoWave — «Подключить к дому»: эта колонка говорит код, его вводят
// здесь; до кнопки — что её настройки и персонажи заменятся домашними, а память объединится. Потом — только комната.
// Всё это делает компьютер-Турбо: из окна колонки (`?via=`) вызовы идут к нему мимо прохода (ownCall).

import { useEffect, useState } from 'preact/hooks';

import { FoundStation, MoveJob, PairCodeAsked, PairCodeLook, can, ownCall, problemOf } from '../api';
import { readHomeDevices } from '../home-devices';
import { tr } from '../i18n';
import { IconSpeaker } from '../icons';
import { MoveProgress } from './placement';

interface Found extends FoundStation {
    /** На колонке свой DuoWave (её /health отвечает как наш шлюз). */
    gateway: boolean;
    port: number;
}

const nameOf = (station: FoundStation): string => station.name ?? station.address;

/** Колонки в сети не в доме: `inHome` — адреса колонок дома (центр, уши). */
export function FoundStations({ inHome, onJoined }: { inHome: Set<string>; onJoined: () => void }) {
    const t = tr.devices;
    const [found, setFound] = useState<Found[]>([]);
    const [open, setOpen] = useState<{ address: string; way: 'ear' | 'code' } | null>(null);
    const [round, setRound] = useState(0);
    const known = [...inHome].sort().join();
    useEffect(() => {
        if (!can('stations.manage')) return;
        let alive = true;
        void (async () => {
            try {
                const { stations } = await ownCall<{ stations: FoundStation[] }>('GET', '/api/v1/stations/connect/discover');
                const outside = stations.filter((station) => !inHome.has(station.address)).slice(0, 6);
                const probed = await Promise.all(
                    outside.map(async (station): Promise<Found> => {
                        const probe = await ownCall<{ gateway: boolean; port: number }>('GET', `/api/v1/placement/probe?address=${encodeURIComponent(station.address)}`).catch(
                            () => null,
                        );
                        return { ...station, gateway: !!probe?.gateway, port: probe?.port ?? 8790 };
                    }),
                );
                if (alive) setFound(probed);
            } catch {
                if (alive) setFound([]);
            }
        })();
        return () => {
            alive = false;
        };
    }, [known, round]);
    if (found.length === 0) return null;
    const joined = (): void => {
        setOpen(null);
        void readHomeDevices();
        setRound((n) => n + 1);
        onJoined();
    };
    return (
        <section class="card glass px-16 py-14 flex flex-col gap-10" aria-labelledby="found-stations-h">
            <h2 id="found-stations-h" class="t-title-xs m-0">
                {t.foundTitle}
            </h2>
            <p class="w-cap m-0">{t.foundLead}</p>
            {found.map((station) => (
                <div key={station.address} class="flex flex-col gap-8">
                    <div class="w-row">
                        <IconSpeaker size={16} />
                        <span class="flex-1 min-w-0 w-strong">{t.stationNamed(nameOf(station))}</span>
                        {open?.address !== station.address &&
                            (station.gateway ? (
                                <button class="w-btn" onClick={() => setOpen({ address: station.address, way: 'code' })}>
                                    {t.joinByCode}
                                </button>
                            ) : station.ssh === null ? (
                                <span class="w-cap">{t.earNeedsRoot}</span>
                            ) : (
                                <button class="w-btn" onClick={() => setOpen({ address: station.address, way: 'ear' })}>
                                    {t.makeEar}
                                </button>
                            ))}
                    </div>
                    {open?.address === station.address &&
                        (open.way === 'ear' ? (
                            <EarMove station={station} onDone={joined} onCancel={() => setOpen(null)} />
                        ) : (
                            <EarByCode station={station} onDone={joined} onCancel={() => setOpen(null)} />
                        ))}
                </div>
            ))}
        </section>
    );
}

/** Без DuoWave: перенос «в ухо» — шаги переноса, как у колонки-центра. */
function EarMove({ station, onDone, onCancel }: { station: Found; onDone: () => void; onCancel: () => void }) {
    const t = tr.devices;
    const [job, setJob] = useState<MoveJob | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        if (!job || job.status !== 'running') {
            if (job?.status === 'done') onDone();
            return;
        }
        const timer = window.setInterval(() => {
            void ownCall<MoveJob>('GET', `/api/v1/placement/move/${encodeURIComponent(job.id)}`)
                .then(setJob)
                .catch(() => undefined);
        }, 1500);
        return () => window.clearInterval(timer);
    }, [job?.id, job?.status]);
    const start = async (): Promise<void> => {
        setError(null);
        try {
            setJob(await ownCall<MoveJob>('POST', '/api/v1/placement/move', { to: 'ear', address: station.address }));
        } catch (e) {
            setError(problemOf(e));
        }
    };
    return (
        <div class="flex flex-col gap-8">
            {job ? (
                <MoveProgress job={job} />
            ) : (
                <>
                    <p class="w-cap m-0">{t.makeEarNote}</p>
                    <div class="flex flex-wrap gap-8">
                        <button type="button" class="btn btn-main" onClick={() => void start()}>
                            {t.makeEar}
                        </button>
                        <button type="button" class="btn btn-ghost" onClick={onCancel}>
                            {t.earCancel}
                        </button>
                    </div>
                </>
            )}
            {error && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}

/** Со своим DuoWave: предупреждение, потом код, который говорит сама колонка (или «Разрешить» в её пульте). */
function EarByCode({ station, onDone, onCancel }: { station: Found; onDone: () => void; onCancel: () => void }) {
    const t = tr.devices;
    const [asked, setAsked] = useState<PairCodeAsked | null>(null);
    const [code, setCode] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        if (!asked) return;
        let alive = true;
        const timer = window.setInterval(() => {
            void ownCall<PairCodeLook>('GET', `/api/v1/placement/pair-code/${encodeURIComponent(asked.id)}`)
                .then((look) => {
                    if (!alive) return;
                    if (look.joined) onDone();
                    else if (look.status === 'denied' || look.status === 'closed') setError(look.why ?? null);
                })
                .catch(() => undefined);
        }, 2500);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, [asked?.id]);
    const ask = async (): Promise<void> => {
        setError(null);
        setBusy(true);
        try {
            setAsked(await ownCall<PairCodeAsked>('POST', '/api/v1/placement/pair-code', { address: station.address, port: station.port, kind: 'ear' }));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const digits = code.replace(/\D/g, '');
    const enter = async (): Promise<void> => {
        if (!asked || digits.length !== 6 || busy) return;
        setBusy(true);
        setError(null);
        try {
            await ownCall('POST', `/api/v1/placement/pair-code/${encodeURIComponent(asked.id)}`, { code: digits });
            onDone();
        } catch (e) {
            setError(problemOf(e));
            setCode('');
        } finally {
            setBusy(false);
        }
    };
    const name = nameOf(station);
    return (
        <div class="flex flex-col gap-8">
            {/* Что будет — до кнопки (memory: checks before the action). */}
            <p class="w-cap m-0">{t.joinEarWarn}</p>
            {asked ? (
                <>
                    <p class="w-cap m-0" role="status">
                        {asked.told?.spoken ? t.earCodeLead(name) : t.earCodeSilent(name)}
                    </p>
                    <label class="flex flex-col gap-6">
                        <span class="w-cap">{t.earCodeLabel}</span>
                        <input
                            class="input"
                            inputMode="numeric"
                            autocomplete="one-time-code"
                            maxLength={8}
                            placeholder="•• •• ••"
                            value={code}
                            disabled={busy}
                            style={{ fontSize: '22px', lineHeight: '26px', letterSpacing: '0.2em', fontVariantNumeric: 'tabular-nums' }}
                            onInput={(event) => setCode((event.target as HTMLInputElement).value)}
                            onKeyDown={(event) => event.key === 'Enter' && void enter()}
                        />
                    </label>
                    <div class="flex flex-wrap gap-8">
                        <button type="button" class="btn btn-main" disabled={busy || digits.length !== 6} onClick={() => void enter()}>
                            {t.joinByCode}
                        </button>
                        <button type="button" class="btn btn-ghost" onClick={onCancel}>
                            {t.earCancel}
                        </button>
                    </div>
                </>
            ) : (
                <div class="flex flex-wrap gap-8">
                    <button type="button" class="btn btn-main" disabled={busy} onClick={() => void ask()}>
                        {t.askCode}
                    </button>
                    <button type="button" class="btn btn-ghost" onClick={onCancel}>
                        {t.earCancel}
                    </button>
                </div>
            )}
            {error && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}
