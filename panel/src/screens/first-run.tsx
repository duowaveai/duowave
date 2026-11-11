// Первый запуск на компьютере (docs/plans/panel-rebuild.md, этап 5): на колонке уже работает DuoWave — выбора размещения
// нет, компьютер становится её помощником одной кнопкой; перенести DuoWave на компьютер — потом, в «Настройках».
// Прежде (docs/plans/desk.md, этап 4; макет Setup): колонки дома, на которых уже работает шлюз
// DuoWave, — и выбор: «Помогать колонке» (советую; окно станет тёмным — так видно, что это помощник) или «Шлюз — на
// этом компьютере» (всё в одном месте, но компьютер спит — колонка молчит). Помощник связывается по паролю пульта
// колонки (placement.tsx, LinkSheet), перенос шлюза сюда — обычным подключением колонки.

import { useEffect, useState } from 'preact/hooks';

import { FoundStation, api, can } from '../api';
import { usePanel } from '../shell/context';
import { LinkSheet } from './placement';

interface Found {
    station: FoundStation;
    version: string | null | undefined;
}

export function FirstRun() {
    const panel = usePanel();
    const [found, setFound] = useState<Found[] | null>(null);
    const [linking, setLinking] = useState<string | null>(null);
    const onPc = panel.health?.placement?.role === 'pc';
    // Компьютер уже связан с колонкой (помощник или шлюз для неё): выбирать больше нечего.
    const linked = !!panel.health?.placement?.station || panel.role.role === 'helper';
    const mayManage = can('stations.manage');
    useEffect(() => {
        if (!onPc || linked || !mayManage) return;
        let alive = true;
        void (async () => {
            try {
                const { stations } = await api.discoverStations();
                const probed = await Promise.all(
                    stations.slice(0, 6).map(async (station): Promise<Found | null> => {
                        const probe = await api.probeStation(station.address).catch(() => null);
                        return probe?.gateway ? { station, version: probe.version } : null;
                    }),
                );
                if (alive) setFound(probed.filter((entry): entry is Found => entry !== null));
            } catch {
                if (alive) setFound([]);
            }
        })();
        return () => {
            alive = false;
        };
    }, [onPc, linked, mayManage]);
    if (!onPc || linked || !found || found.length === 0) return null;
    const first = found[0];
    const name = first.station.name ?? first.station.address;
    return (
        <section class="card glass first-run" aria-labelledby="first-run-h">
            <h2 id="first-run-h" class="t-title m-0">
                На колонке «{name}» уже работает DuoWave
            </h2>
            <p class="t-caption m-0">
                Колонка отвечает сама, даже когда компьютер спит. Этот компьютер станет её помощником: слух, голос, модель, копии
                настроек и мастерская навыков. Перенести DuoWave на компьютер можно позже: «Настройки» → «Колонка».
            </p>
            <div>
                <button class="w-btn w-btn-ink" onClick={() => setLinking(first.station.address)}>
                    Помогать колонке
                </button>
            </div>
            {found.length > 1 && <p class="t-caption m-0">Ещё колонки со шлюзом: {found.slice(1).map((entry) => entry.station.name ?? entry.station.address).join(', ')}.</p>}
            {linking && (
                <LinkSheet
                    initialAddress={linking}
                    onClose={() => setLinking(null)}
                    onLinked={() => {
                        setLinking(null);
                        panel.go('home');
                    }}
                />
            )}
        </section>
    );
}
