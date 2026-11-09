import { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { AccessCatalog, Health, ModeView, PanelMode, RequestView, SessionSnapshot, UnsupportedEntry, api, can, plural, problemOf } from '../api';
import { FeatureId, available, useAvailability, whyNot } from '../availability';
import { CAPABILITIES, CAPABILITY_TITLES } from '../capabilities';
import { executorOf } from '../executor';
import { StationDrawing } from '../stations';
import { timeAgo } from '../time';
import { ConnectFirstCard } from './connect';
import { Dashboard } from './dashboard';
import { Slot } from '../shell/slot';
import { RequestRow } from './journal';
import type { SettingsTab } from './mind';

interface Props {
    health: Health | null;
    debug: { sessions: SessionSnapshot[] | null; unsupported: UnsupportedEntry[]; error: unknown };
    catalog: AccessCatalog | null;
    /** Сессия выбранной колонки и её рисунок — для середины дашборда. */
    session?: SessionSnapshot;
    drawing: StationDrawing;
    online: boolean;
    /** Настоящий запрос колонки из живого канала и часы шлюза: путь вопроса на дашборде. */
    trip?: RequestView;
    skew?: number;
    /** Растёт с каждой переменой на шлюзе: дашборд перечитывает своё. */
    version?: number;
    /** Последние запросы этой колонки из живого канала (только тому, кто может читать журнал). */
    recent?: RequestView[];
    /** Плашка о новой колонке над режимом, если есть что настроить. */
    banner?: ComponentChildren;
    /** Колонок ещё нет: вместо дашборда — «Подключите колонку». */
    noStation?: boolean;
    onConnect?: () => void;
    onOpenJournal: () => void;
    onOpenSettings: (tab: SettingsTab) => void;
    /** В «Дом» → «Коннекторы», сразу в лист этого подключения, если оно названо. */
    onOpenHouse: (connector?: string) => void;
}

const MODES: Array<{ value: PanelMode; label: string; feature: FeatureId }> = [
    { value: 'relay', label: 'Яндекс', feature: 'mode.relay' },
    { value: 'gateway', label: 'Шлюз', feature: 'mode.gateway' },
    { value: 'own', label: 'Свой', feature: 'mode.own' },
];

const MODE_LABELS: Record<PanelMode, string> = { relay: 'Яндекс', gateway: 'Шлюз', own: 'Свой' };

/**
 * Шлюз работает на колонке, а это пульт компьютера: режим, дашборд и журнал колонки — в её пульте; здесь — шлюз
 * компьютера, её домашний сервер.
 */
function OnStationNote({ health }: { health: Health | null }) {
    const station = health?.placement?.role === 'pc' ? health.placement.station : undefined;
    if (!station || station.placement === 'pc') return null;
    return (
        <div class="card glass py-12 px-16 flex gap-10 items-center flex-wrap" role="status">
            <span class="t-caption flex-1 min-w-200">
                Шлюз работает на колонке{station.reachable ? '' : ' — и сейчас не отвечает'}. Её режим, дашборд и журнал — в её пульте;
                здесь — шлюз компьютера{station.placement === 'stationServer' ? ', её домашний сервер' : ''}.
            </span>
            <a class="pill pill-off py-8 px-14" href={`http://${station.address}:${station.port ?? 8790}/panel`} target="_blank" rel="noreferrer">
                Пульт колонки ↗
            </a>
        </div>
    );
}

/** «с 14:02»: время перехода сегодня, иначе с датой. */
function since(at: string): string {
    const date = new Date(at);
    const time = date.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    return date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString('ru-RU')} ${time}`;
}

/** Режим колонки: сегмент из макета. Переключение закрывает соединение — колонка переподключается в новый режим. */
function ModeCard({
    health,
    onOpenSettings,
    detailsOpen,
    onToggleDetails,
}: {
    health: Health | null;
    onOpenSettings: () => void;
    /** Строка «Слышит · Думает · Говорит» под сегментом: слово в конце подписи режима раскрывает её.
     * Состояние общее с дашбордом — держит HomeScreen. */
    detailsOpen: boolean;
    onToggleDetails: () => void;
}) {
    const [mode, setMode] = useState<PanelMode | null>(health?.panelMode ?? null);
    const [note, setNote] = useState<string>('');
    const [voice, setVoice] = useState<'yandex' | 'own'>('own');
    /** Переход, который шлюз сделал сам (компьютер пропал): объяснение и «Остаться». */
    const [auto, setAuto] = useState<ModeView['auto']>(undefined);
    // Что где доступно: перечитывается при каждой перемене на шлюзе — кнопки режимов следуют за ним.
    useAvailability();
    const [busy, setBusy] = useState(false);
    const [reconnecting, setReconnecting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [warn, setWarn] = useState(false);
    /** Нажат режим, которого в этой сборке нет: пояснение вместо переключения. */
    const [unavailable, setUnavailable] = useState<string | null>(null);
    const mayPick = can('station.mode');
    // Для интервала: свежие флаги, а не те, что были при его создании.
    const quiet = useRef(true);
    quiet.current = !busy && !reconnecting;
    // Связь колонки до переключения: «вернулась» — это переход «нет связи → есть».
    const wasConnected = useRef<boolean | undefined>(undefined);

    useEffect(() => {
        const read = () =>
            api
                .mode()
                .then((current) => {
                    if (!quiet.current) return;
                    setMode(current.mode);
                    setNote(current.note);
                    setVoice(current.voice);
                    setAuto(current.by === 'auto' ? current.auto : undefined);
                })
                .catch(() => {});
        void read();
        // Режим и голос меняются и с других устройств, и из экрана «Голос».
        const id = setInterval(read, 5000);
        return () => clearInterval(id);
    }, []);

    /** Переключение занимает секунду-другую: сегмент отвечает сразу, кнопки ждут, ошибка возвращает как было. */
    const pick = async (next: PanelMode) => {
        if (!mayPick || busy || next === mode) return;
        // Режим, которого здесь сейчас нет («Свой» на колонке без компьютера): объясняем словами шлюза.
        const feature = MODES.find((option) => option.value === next)?.feature;
        if (feature && !available(feature)) return setUnavailable(whyNot(feature) ?? 'Этот режим здесь сейчас не включить.');
        setUnavailable(null);
        // «Свой» без своего голоса — сначала предупреждение: ответы уйдут Яндексу.
        if (next === 'own' && voice === 'yandex') return setWarn(true);
        await doSwitch(next);
    };

    const doSwitch = async (next: PanelMode) => {
        const before = mode;
        setWarn(false);
        setBusy(true);
        setError(null);
        setMode(next);
        try {
            const switched = await api.setMode(next);
            setMode(switched.mode);
            setNote(switched.note);
            setVoice(switched.voice);
            setAuto(undefined);
            wasConnected.current = true;
            setReconnecting(true);
        } catch (e) {
            setMode(before);
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    // Колонка переподключается после разрыва — как вернётся (было «нет связи», стало «есть»), объяснение уходит.
    useEffect(() => {
        const connected = health?.station?.connected;
        if (!reconnecting) return;
        if (connected === false) wasConnected.current = false;
        if (connected && wasConnected.current === false) setReconnecting(false);
    }, [health, reconnecting]);
    // Не вернулась за 20 секунд — объяснение уходит. Свой таймер: свежий /health его не сбрасывает.
    useEffect(() => {
        if (!reconnecting) return;
        const giveUp = setTimeout(() => setReconnecting(false), 20_000);
        return () => clearTimeout(giveUp);
    }, [reconnecting]);

    return (
        <div>
            <div
                role="radiogroup"
                aria-label="Режим колонки"
                aria-busy={busy}
                class="glass grid grid-cols-3 gap-4 p-4 r-pill"
            >
                {MODES.map((option) => {
                    const chosen = mode === option.value;
                    const allowed = available(option.feature);
                    return (
                        <button
                            key={option.value}
                            role="radio"
                            aria-checked={chosen}
                            // Недоступный режим остаётся нажимаемым: нажатие объясняет, почему его нет.
                            aria-disabled={!allowed || undefined}
                            title={allowed ? undefined : whyNot(option.feature)}
                            disabled={!mayPick || busy}
                            onClick={() => void pick(option.value)}
                            // Выбранный — лайм в тёмной теме и чернила в светлой (DESIGN.md: The Ink-Selection Rule): .seg-btn.
                            class="seg-btn"
                        >
                            {option.label}
                        </button>
                    );
                })}
            </div>
            <p class="t-meta mx-6 mt-8 mb-0 flex items-center gap-8">
                {(busy || reconnecting) && <span class="spinner" aria-hidden="true" />}
                <span>
                    {busy
                        ? 'Переключаю — колонка переподключится за секунды…'
                        : reconnecting
                          ? 'Колонка переподключается в новый режим…'
                          : note}
                    {!busy && !reconnecting && (
                        <>
                            {' '}
                            <button class="truth-more" aria-expanded={detailsOpen} onClick={onToggleDetails}>
                                {detailsOpen ? 'свернуть' : 'подробнее'}
                            </button>
                        </>
                    )}
                    {!mayPick && !busy && !reconnecting && ' Переключает администратор.'}
                    {!available('mode.own') && !auto && !busy && !reconnecting && ` ${whyNot('mode.own') ?? ''}`}
                </span>
            </p>
            {auto && mode === auto.to && (
                <div class="t-caption mx-6 mt-8 mb-0 flex gap-8 items-center flex-wrap" role="status">
                    <span>
                        Сейчас «{MODE_LABELS[auto.to]}»: {auto.reason} с {since(auto.at)}. Вернусь в «{MODE_LABELS[auto.from]}», когда он
                        появится.
                    </span>
                    {mayPick && (
                        <button
                            class="pill pill-off pill-btn-sm"
                            onClick={() =>
                                void api
                                    .keepMode()
                                    .then(() => setAuto(undefined))
                                    .catch((e) => setError(problemOf(e)))
                            }
                        >
                            Остаться в «{MODE_LABELS[auto.to]}»
                        </button>
                    )}
                </div>
            )}
            {unavailable && (
                <p class="t-caption mx-6 mt-8 mb-0" role="status">
                    {unavailable}
                </p>
            )}
            {warn && (
                <div role="alert" class="mx-6 mt-10 mb-0 py-12 px-14 r-panel bg-pill-yandex-bg c-pill-yandex-fg">
                    <p class="m-0 fs-13 lh-18 fw-700">
                        Выбран голос Алисы — без Яндекса он не звучит
                    </p>
                    <p class="m-0 mt-4 mb-10 fs-13 lh-18">
                        В режиме «Свой» вместо него будет говорить наш голос (или колонка замолчит, если своего голосового
                        сервера нет). Выберите свой голос — или продолжите так.
                    </p>
                    <div class="flex gap-8 flex-wrap">
                        <button class="pill pill-own pill-btn" onClick={onOpenSettings}>
                            Выбрать свой голос
                        </button>
                        <button class="pill pill-yandex pill-btn" onClick={() => void doSwitch('own')}>
                            Продолжить
                        </button>
                        <button class="pill pill-off pill-btn" onClick={() => setWarn(false)}>
                            Отмена
                        </button>
                    </div>
                </div>
            )}
            {error && (
                <p class="mx-6 mt-4 mb-0 fs-13 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}

function AllocPanel({ health, catalog }: { health: Health | null; catalog: AccessCatalog | null }) {
    // Режим пульта; шлюз постарше сообщает только вид сессии — relay или gateway.
    const mode: PanelMode = health?.panelMode ?? (health?.mode === 'relay' ? 'relay' : 'gateway');
    const routes = health?.skills?.routes;
    // В «Яндексе» маршруты не действуют, даже если шлюз их прислал: полоса из них соврала бы.
    if (!routes || mode === 'relay') {
        return (
            <section class="card glass flex flex-col gap-12" aria-labelledby="alloc-h">
                <h2 id="alloc-h" class="t-title-xs m-0">
                    Кто отвечает
                </h2>
                <p class="t-meta m-0">
                    {mode === 'relay'
                        ? 'Режим «Яндекс»: отвечает Яндекс, шлюз только наблюдает.'
                        : 'Шлюз прогревается — маршруты появятся через секунду.'}
                </p>
            </section>
        );
    }

    const counts = { own: 0, yandex: 0, off: 0 };
    // Каталог возможностей — шлюза (новая возможность видна сразу); без него — то, что знает пульт.
    const names = catalog?.capabilities.map((capability) => capability.id) ?? CAPABILITIES.map((meta) => meta.name);
    for (const name of names) {
        counts[executorOf(routes[name], mode)] += 1;
    }
    const total = counts.own + counts.yandex + counts.off;
    const legend: Array<{ key: 'own' | 'yandex' | 'off'; label: string }> = [
        { key: 'own', label: 'свой' },
        { key: 'yandex', label: 'Яндекс' },
        { key: 'off', label: 'выкл' },
    ];

    return (
        <section class="card glass flex flex-col gap-12" aria-labelledby="alloc-h">
            <div class="flex items-baseline justify-between">
                <h2 id="alloc-h" class="t-title-xs m-0">
                    Кто отвечает
                </h2>
                <span class="t-meta">
                    {total} {plural(total, 'возможность', 'возможности', 'возможностей')}
                </span>
            </div>
            <div class="alloc-bar" aria-hidden="true">
                {legend.map(({ key }) =>
                    counts[key] > 0 ? (
                        <span key={key} style={{ flex: `${counts[key]} 1 0`, background: `var(--bar-${key})` }} />
                    ) : null,
                )}
            </div>
            <div class="alloc-legend">
                {legend.map(({ key, label }) => (
                    <span key={key}>
                        <span class="alloc-dot" aria-hidden="true" style={{ background: `var(--bar-${key})` }} />
                        <span class="t-meta">{label}</span>
                        <b>{counts[key]}</b>
                    </span>
                ))}
            </div>
            <ServedTotals health={health} />
        </section>
    );
}

/** Сколько запросов кто обслужил — живые числа, двигаются с каждой фразой (счёт с запуска шлюза). */
function ServedTotals({ health }: { health: Health | null }) {
    const served = health?.skills?.served;
    if (!served) return null;
    const totals = { own: 0, yandex: 0, unsupported: 0 };
    for (const counts of Object.values(served))
        for (const key of Object.keys(totals) as Array<keyof typeof totals>) totals[key] += counts[key] ?? 0;
    const total = totals.own + totals.yandex + totals.unsupported;
    if (!total) return <p class="t-caption m-0">Скажите что-нибудь колонке — здесь появится, кто отвечал.</p>;
    return (
        <p class="t-caption m-0">
            обслужено с запуска: своими — {totals.own} · Яндексом — {totals.yandex}
            {totals.unsupported ? ` · не смогли — ${totals.unsupported}` : ''}
        </p>
    );
}

/** Последние запросы под дашбордом: что спросили, что ответили, кто делал шаги; весь журнал — по кнопке. */
function RecentRequests({
    list,
    catalog,
    onOpenJournal,
}: {
    list: RequestView[];
    catalog: AccessCatalog | null;
    onOpenJournal: () => void;
}) {
    const [open, setOpen] = useState<string | null>(null);
    if (list.length === 0) return null;
    const titles: Record<string, string> = catalog
        ? Object.fromEntries(catalog.capabilities.map((capability) => [capability.id, capability.title]))
        : CAPABILITY_TITLES;
    return (
        <section class="card glass py-4 px-16" aria-labelledby="recent-h">
            <div class="card-head">
                <h2 id="recent-h" class="t-title-xs m-0">
                    Последние запросы
                </h2>
                <button class="fs-14 fw-700 c-accent-text min-h-44" onClick={onOpenJournal}>
                    Журнал
                </button>
            </div>
            <ol class="req-list">
                {list.map((request) => (
                    <RequestRow
                        key={request.id}
                        request={request}
                        titles={titles}
                        open={open === request.id}
                        onToggle={() => setOpen(open === request.id ? null : request.id)}
                    />
                ))}
            </ol>
        </section>
    );
}

function UnsupportedTop({ list, onOpenJournal }: { list: UnsupportedEntry[]; onOpenJournal: () => void }) {
    if (list.length === 0) return null;
    return (
        <section class="card glass card-list card-list-tight" aria-labelledby="unsupported-h">
            <div class="card-head">
                <h2 id="unsupported-h" class="t-title-xs m-0">
                    Пока не умеем
                </h2>
                <button class="fs-14 fw-700 c-accent-text min-h-44" onClick={onOpenJournal}>
                    Все {list.length}
                </button>
            </div>
            {list.slice(0, 3).map((entry) => (
                <div key={entry.at + entry.text} class="flex flex-col gap-2 py-8 px-0 bt-divider">
                    <span class="fw-700 overflow-hidden text-ellipsis nowrap">
                        «{entry.text}»
                    </span>
                    <span class="t-caption">
                        {timeAgo(entry.at)} · {entry.reason}
                    </span>
                </div>
            ))}
        </section>
    );
}

/** «Колонка»: режим, дашборд с орбитой, кто отвечает и чего пока не умеем — ниже, прокруткой. */
export function HomeScreen({
    health,
    debug,
    catalog,
    session,
    drawing,
    online,
    trip,
    skew,
    version,
    recent,
    banner,
    noStation,
    onConnect,
    onOpenJournal,
    onOpenSettings,
    onOpenHouse,
}: Props) {
    // Строка «Слышит · Думает · Говорит» на дашборде: скрыта по умолчанию, открывается словом «подробнее» в конце
    // подписи режима или янтарного статуса. Хранится здесь: оба предложения — одна кнопка.
    const [whoOpen, setWhoOpen] = useState(false);
    const toggleWho = (): void => setWhoOpen((open) => !open);
    // Без колонки дашборду нечего показать: режимы и орбита были бы о колонке, которой нет.
    if (noStation && onConnect)
        return (
            <>
                {banner}
                <ConnectFirstCard onConnect={onConnect} />
            </>
    );
    return (
        <>
            {banner}
            <OnStationNote health={health} />
            <ModeCard health={health} onOpenSettings={() => onOpenSettings('voice')} detailsOpen={whoOpen} onToggleDetails={toggleWho} />
            <Dashboard
                health={health}
                session={session}
                drawing={drawing}
                online={online}
                trip={trip}
                skew={skew}
                version={version}
                onOpenSettings={onOpenSettings}
                onOpenHouse={onOpenHouse}
                whoOpen={whoOpen}
                onToggleWho={toggleWho}
            />
            {recent && <RecentRequests list={recent} catalog={catalog} onOpenJournal={onOpenJournal} />}
            <AllocPanel health={health} catalog={catalog} />
            <UnsupportedTop list={debug.unsupported} onOpenJournal={onOpenJournal} />
            <Slot name="home.cards" />
        </>
    );
}
