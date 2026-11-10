// Первый запуск на компьютере (docs/plans/desk.md, этап 4; макет Setup): колонки дома, на которых уже работает шлюз
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
                На колонке «{name}» уже работает шлюз
            </h2>
            <p class="t-meta m-0">DuoWave на этом компьютере может ей помогать — или забрать шлюз себе. Решение можно поменять потом.</p>
            <div class="first-run-options">
                <div class="first-run-option is-helper">
                    <div class="flex items-center gap-8">
                        <b>Помогать колонке</b>
                        <span class="pill pill-own">советую</span>
                    </div>
                    <p class="t-caption m-0">
                        Колонка — шлюз и отвечает сама, даже когда компьютер спит. Компьютер — помощник: слух, голос и модель, копии её
                        настроек, мастерская плагинов.
                    </p>
                    <button class="w-btn w-btn-ink" onClick={() => setLinking(first.station.address)}>
                        Стать помощником
                    </button>
                    <p class="t-caption m-0">Окно станет тёмным: так сразу видно, что это помощник.</p>
                </div>
                <div class="first-run-option">
                    <b>Шлюз — на этом компьютере</b>
                    <p class="t-caption m-0">
                        + Всё в одном месте, в том числе звонки и MCP-программы. − Компьютер спит — колонка молчит. Колонку перенастрою на
                        этот компьютер: перезагрузка, около минуты без голоса.
                    </p>
                    <button class="w-btn" onClick={panel.openConnect}>
                        Перенести шлюз сюда…
                    </button>
                    <p class="t-caption m-0">Окно останется белым: шлюз — здесь.</p>
                </div>
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
