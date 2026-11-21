// Первые шаги → «Что даёт этот компьютер» (docs/plans/home-setup-flow.md, этап 3; холст «Онбординг», доска
// TurboGives): компьютер только что вошёл в дом по коду и сразу помогает колонке-центру. Здесь — что он даёт (слух,
// голос, модель, сообщения: те же переключатели, что у Турбо на столе), «Не давать компьютеру спать» (в приложении) и
// связь с колонкой: достаёт ли она до компьютера, а на Windows — «Разрешить в брандмауэре». Дальше — стол дома.

import { useEffect, useState } from 'preact/hooks';

import { HelperPart, StationGatewayView, api, can, problemOf } from '../../api';
import { tr } from '../../i18n';
import { Switch } from '../../ui/switch';
import { AllowFirewall } from '../connect';
import { KeepAwake, parts } from '../helper-pc';

export function TurboStep({ station, name, onDone }: { station: string; name: string; onDone: () => void }) {
    const t = tr.onboarding;
    const [view, setView] = useState<StationGatewayView | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const read = async (): Promise<void> => {
        const placement = await api.placement(station).catch(() => null);
        if (placement?.station) setView(placement.station);
    };
    // Связь колонки с компьютером видна не сразу: колонка смотрит на него раз в несколько секунд.
    useEffect(() => {
        void read();
        const timer = window.setInterval(() => void read(), 5000);
        return () => window.clearInterval(timer);
    }, [station]);
    const helping = view?.record?.placement === 'stationServer';
    const home = view?.live.health?.placement?.homeServer;
    const without: HelperPart[] = home?.without ?? [];
    const mayManage = can('stations.manage');
    const run = (what: string, job: () => Promise<StationGatewayView | unknown>): void => {
        setBusy(what);
        setError(null);
        void job()
            .then(() => read())
            .catch((e) => setError(problemOf(e)))
            .finally(() => setBusy(null));
    };
    const link = !helping ? t.turboHelpOff : home?.state === 'up' ? t.turboLinkUp : home?.state === 'down' ? t.turboLinkDown : t.turboLinkWait;
    return (
        <div class="onb-split">
            <section class="onb-col" style={{ maxWidth: '540px', gap: '16px' }}>
                <h1 class="h2">{t.turboTitle(name)}</h1>
                <p class="lead" style={{ fontSize: '15px', lineHeight: 1.55 }}>
                    {t.turboAbout}
                </p>
                <div class="card card-tight" style={{ padding: '6px 18px' }}>
                    {parts().map((part) => {
                        const on = helping && !without.includes(part.id);
                        return (
                            <label key={part.id} class="w-toggle" style={{ padding: '10px 0' }}>
                                <span class="flex-1 min-w-0 flex flex-col">
                                    <span class="w-strong">{part.title}</span>
                                    <span class="w-cap">{tr.desk.otherwise(part.without)}</span>
                                </span>
                                <Switch
                                    on={on}
                                    label={tr.desk.giveToStation(part.title.toLowerCase())}
                                    disabled={!mayManage || busy !== null || !helping}
                                    onChange={(next) =>
                                        run(part.id, () => api.setHelperParts(next ? without.filter((id) => id !== part.id) : [...without, part.id], station))
                                    }
                                />
                            </label>
                        );
                    })}
                    <KeepAwake />
                </div>
            </section>

            <section class="card onb-col" style={{ maxWidth: '480px', gap: '14px', padding: '24px' }}>
                <h2 class="h3">{t.turboLink(name)}</h2>
                <p class="small" role="status" style={{ margin: 0 }}>
                    {link}
                </p>
                {!helping && mayManage && (
                    <button type="button" class="dw-btn dw-btn-light" disabled={busy !== null} onClick={() => run('help', () => api.setStationHelper(true, station))}>
                        {t.turboHelpOn}
                    </button>
                )}
                {/* Только на Windows: входящие к DuoWave из домашней сети (готовые проверки сети Windows). */}
                <AllowFirewall onAllowed={() => void read()} />
                {error && (
                    <p class="small" role="alert" style={{ margin: 0, color: 'var(--bad)' }}>
                        {error}
                    </p>
                )}
                <div class="onb-row-between">
                    <span />
                    <button type="button" class="dw-btn dw-btn-lg dw-btn-accent" onClick={onDone}>
                        {t.turboToDesk}
                    </button>
                </div>
            </section>
        </div>
    );
}
