import { useEffect, useState } from 'preact/hooks';

import { AccessCatalog, Station, UpdateStatus, api, can, plural, problemOf } from '../api';
import { IconLinks, IconSpeaker } from '../icons';
import { timeAgo } from '../time';
import { AskSheet } from './ask';
import { Connectors } from './connectors';
import { McpSection } from './mcp';
import type { SettingsTab } from './mind';
import { StationMemoryCard } from './memory';
import { PlacementCard } from './placement';
import { Contribution, useSlot } from '../shell/slot';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { checkIsStale, setUpdateStatus, useOfferedVersion } from '../updates';
import { ask } from '../ui/confirm';

export type HouseTab = 'connectors' | 'gateway';

/**
 * «Дом»: всё, что вокруг колонки, вкладками (и вкладками плагинов из слота house.tabs). «Коннекторы» — что подключено дома и что уходит в интернет, и
 * где каждое настраивается (своё — листом здесь), MCP-серверы; на колонке первым — её домашний сервер. «Шлюз» — где
 * он работает (компьютер, колонка, колонка с домашним сервером) и как туда перейти, каким колонкам он отвечает и какая
 * версия стоит. Пакеты и каталог — в разделе «Плагины» (screens/plugins-section.tsx). Люди, ключи, свой вход, история
 * изменений и вид пульта — раздел «Настройки» (screens/settings.tsx).
 */
export function HouseScreen({
    catalog,
    tab,
    onTab,
    onOpenSettings,
    openConnector,
    onConnectorOpened,
}: {
    catalog: AccessCatalog | null;
    /** Встроенная вкладка (HouseTab) или вкладка плагина — `плагин.вклад`. */
    tab: string;
    onTab: (tab: string) => void;
    onOpenSettings: (tab: SettingsTab) => void;
    /** Лист подключения, который открыть сразу (ссылка с дашборда). */
    openConnector?: string | null;
    onConnectorOpened?: () => void;
}) {
    const [asking, setAsking] = useState(false);
    const offered = useOfferedVersion();
    // Вкладки плагинов — после своих; у каждой своя граница ошибок.
    const extra = useSlot('house.tabs');
    const tabs: Array<TabEntry<string>> = [
        ...(can('connectors.view') ? [{ id: 'connectors', label: 'Коннекторы', icon: <IconLinks size={16} /> }] : []),
        ...(can('station.view') ? [{ id: 'gateway', label: 'Шлюз', icon: <IconSpeaker size={16} />, dot: !!offered }] : []),
        ...extra.map((entry) => ({ id: entry.key, label: entry.title ?? entry.id, icon: entry.icon ? <entry.icon size={16} /> : null })),
    ];
    // Вкладка, которой этому входу не видно, — первая из видных.
    const shown = tabs.some((entry) => entry.id === tab) ? tab : tabs[0].id;
    return (
        <>
            <SectionTabs label="Дом" tabs={tabs} tab={shown} onPick={onTab} />
            {shown === 'connectors' && (
                <>
                    <Connectors onOpenSettings={onOpenSettings} openId={openConnector} onOpened={onConnectorOpened} />
                    <McpSection onAsk={() => setAsking(true)} />
                    {asking && <AskSheet onClose={() => setAsking(false)} />}
                </>
            )}
            {shown === 'gateway' && (
                <>
                    <PlacementCard />
                    <StationMemoryCard />
                    <Stations />
                    <Updates />
                </>
            )}
            {extra.map(
                (entry) =>
                    shown === entry.key && (
                        <Contribution
                            key={entry.key}
                            entry={entry}
                            fallback={
                                <section class="card glass" role="alert">
                                    <p class="t-meta m-0">
                                        Вкладка «{entry.title ?? entry.id}» не открылась — ошибка в плагине «{entry.plugin}» (подробности в консоли
                                        браузера). Остальной «Дом» работает.
                                    </p>
                                </section>
                            }
                        />
                    ),
            )}
        </>
    );
}

const STATION_PILL: Record<Station['status'], { cls: string; label: string }> = {
    approved: { cls: 'pill-own', label: 'отвечаем' },
    pending: { cls: 'pill-yandex', label: 'ждёт разрешения' },
    blocked: { cls: 'pill-off', label: 'не пускаем' },
};

/** Колонки: первая подключилась сама, каждую следующую пускает администратор. */
function Stations() {
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
                title: `Не пускать «${station.name}»?`,
                text: 'Она перестанет отвечать, пока вы её не разрешите.',
                ok: 'Не пускать',
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
                    Колонки
                </h2>
                {list && (
                    <span class="t-meta">
                        {list.length} {plural(list.length, 'колонка', 'колонки', 'колонок')}
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
                    Ни одна колонка ещё не подключалась.
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
                            {station.lastAddress ?? 'адрес неизвестен'} · была {timeAgo(station.lastSeen)}
                        </span>
                    </div>
                    <span class={`pill ${STATION_PILL[station.status].cls}`}>{STATION_PILL[station.status].label}</span>
                    {mayManage && station.status !== 'approved' && (
                        <button
                            class="pill pill-own pill-btn"
                            disabled={busy === station.device}
                            onClick={() => void set(station, 'approved')}
                        >
                            разрешить
                        </button>
                    )}
                    {mayManage && station.status === 'approved' && (
                        <button
                            class="pill pill-off pill-btn"
                            disabled={busy === station.device}
                            onClick={() => void set(station, 'blocked')}
                        >
                            не пускать
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
function Updates() {
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
                setMessage({ ok: true, text: answer.error ? `Лента не ответила: ${answer.error}` : 'Стоит последняя версия.' });
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const install = async () => {
        if (!status.available) return;
        const yes = await ask({
            title: `Поставить версию ${status.available.version}?`,
            text: 'Шлюз перезапустится, колонка переподключится сама.',
            ok: 'Поставить',
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
                    Обновления
                </h2>
                <span class="t-meta">версия {status.current}</span>
            </div>
            {offer ? (
                <div>
                    <p class="t-body m-0 mb-4">
                        Доступна версия <b>{offer.version}</b>
                        {offer.date ? `, ${new Date(offer.date).toLocaleDateString('ru-RU')}` : ''}
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
                        ? 'Лента выпусков не задана: шлюз не проверяет обновления.'
                        : checking
                          ? 'Проверяю, нет ли новой версии…'
                          : status.checkedAt
                          ? `Новых версий нет · проверено ${timeAgo(status.checkedAt)}`
                          : 'Ещё не проверял.'}
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
                    Ставится. Шлюз перезапустится и вернётся через несколько секунд.
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
                        проверить
                    </button>
                    {status.canInstall && (
                        <button
                            class="pill pill-own pill-btn"
                            disabled={busy || status.installing}
                            onClick={() => void install()}
                        >
                            {status.installing ? 'ставится…' : 'обновить'}
                        </button>
                    )}
                </div>
            )}
        </section>
    );
}
