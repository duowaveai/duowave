// Карточки «Колонки», которые встают на стол виджетами (screens/desk-widgets.tsx): режим колонки, кто отвечает и
// сколько обслужил, последние запросы, чего пока не умеем.

import { useEffect, useRef, useState } from 'preact/hooks';

import { AccessCatalog, Health, ModeView, PanelMode, RequestView, UnsupportedEntry, api, can, problemOf } from '../api';
import { FeatureId, available, useAvailability, whyNot } from '../availability';
import { CAPABILITIES, CAPABILITY_TITLES } from '../capabilities';
import { executorOf } from '../executor';
import { locale, tr } from '../i18n';
import { timeAgo } from '../time';
import { RequestRow } from './journal';

const modeLabels = (): Record<PanelMode, string> => ({ relay: tr.home.modeRelay, gateway: tr.home.modeGateway, own: tr.home.modeOwn });

const modes = (): Array<{ value: PanelMode; label: string; feature: FeatureId }> => [
    { value: 'relay', label: modeLabels().relay, feature: 'mode.relay' },
    { value: 'gateway', label: modeLabels().gateway, feature: 'mode.gateway' },
    { value: 'own', label: modeLabels().own, feature: 'mode.own' },
];

/** «с 14:02»: время перехода сегодня, иначе с датой. */
function since(at: string): string {
    const date = new Date(at);
    const time = date.toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
    return date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString(locale())} ${time}`;
}

/** Режим колонки: сегмент из макета. Переключение закрывает соединение — колонка переподключается в новый режим. */
export function ModeCard({
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
        const feature = modes().find((option) => option.value === next)?.feature;
        if (feature && !available(feature)) return setUnavailable(whyNot(feature) ?? tr.home.modeUnavailable);
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
                aria-label={tr.home.modeGroup}
                aria-busy={busy}
                class="glass grid grid-cols-3 gap-4 p-4 r-pill"
            >
                {modes().map((option) => {
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
                        ? tr.home.switching
                        : reconnecting
                          ? tr.home.reconnecting
                          : note}
                    {!busy && !reconnecting && (
                        <>
                            {' '}
                            <button class="truth-more" aria-expanded={detailsOpen} onClick={onToggleDetails}>
                                {detailsOpen ? tr.home.collapse : tr.home.more}
                            </button>
                        </>
                    )}
                    {!mayPick && !busy && !reconnecting && tr.home.adminSwitches}
                    {!available('mode.own') && !auto && !busy && !reconnecting && ` ${whyNot('mode.own') ?? ''}`}
                </span>
            </p>
            {auto && mode === auto.to && (
                <div class="t-caption mx-6 mt-8 mb-0 flex gap-8 items-center flex-wrap" role="status">
                    <span>
                        {tr.home.autoNow(modeLabels()[auto.to], auto.reason, since(auto.at), modeLabels()[auto.from])}
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
                            {tr.home.stayIn(modeLabels()[auto.to])}
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
                        {tr.home.warnTitle}
                    </p>
                    <p class="m-0 mt-4 mb-10 fs-13 lh-18">
                        {tr.home.warnText}
                    </p>
                    <div class="flex gap-8 flex-wrap">
                        <button class="pill pill-own pill-btn" onClick={onOpenSettings}>
                            {tr.home.pickOwnVoice}
                        </button>
                        <button class="pill pill-yandex pill-btn" onClick={() => void doSwitch('own')}>
                            {tr.home.proceed}
                        </button>
                        <button class="pill pill-off pill-btn" onClick={() => setWarn(false)}>
                            {tr.home.cancel}
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

export function AllocPanel({ health, catalog }: { health: Health | null; catalog: AccessCatalog | null }) {
    // Режим пульта; шлюз постарше сообщает только вид сессии — relay или gateway.
    const mode: PanelMode = health?.panelMode ?? (health?.mode === 'relay' ? 'relay' : 'gateway');
    const routes = health?.skills?.routes;
    // В «Яндексе» маршруты не действуют, даже если шлюз их прислал: полоса из них соврала бы.
    if (!routes || mode === 'relay') {
        return (
            <section class="card glass flex flex-col gap-12" aria-labelledby="alloc-h">
                <h2 id="alloc-h" class="t-title-xs m-0">
                    {tr.home.whoAnswered}
                </h2>
                <p class="t-meta m-0">
                    {mode === 'relay'
                        ? tr.home.relayObserves
                        : tr.home.warmingUp}
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
        { key: 'own', label: tr.home.legendOwn },
        { key: 'yandex', label: tr.home.legendYandex },
        { key: 'off', label: tr.home.legendOff },
    ];

    return (
        <section class="card glass flex flex-col gap-12" aria-labelledby="alloc-h">
            <div class="flex items-baseline justify-between">
                <h2 id="alloc-h" class="t-title-xs m-0">
                    {tr.home.whoAnswered}
                </h2>
                <span class="t-meta">
                    {tr.home.capabilities(total)}
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
    if (!total) return <p class="t-caption m-0">{tr.home.sayToStation}</p>;
    return (
        <p class="t-caption m-0">
            {tr.home.servedSince(totals.own, totals.yandex)}
            {totals.unsupported ? tr.home.couldNot(totals.unsupported) : ''}
        </p>
    );
}

/** Последние запросы под дашбордом: что спросили, что ответили, кто делал шаги; весь журнал — по кнопке. */
export function RecentRequests({
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
                    {tr.home.recentRequests}
                </h2>
                <button class="fs-14 fw-700 c-accent-text min-h-44" onClick={onOpenJournal}>
                    {tr.home.conversationHistory}
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

export function UnsupportedTop({ list, onOpenJournal }: { list: UnsupportedEntry[]; onOpenJournal: () => void }) {
    if (list.length === 0) return null;
    return (
        <section class="card glass card-list card-list-tight" aria-labelledby="unsupported-h">
            <div class="card-head">
                <h2 id="unsupported-h" class="t-title-xs m-0">
                    {tr.home.notYet}
                </h2>
                <button class="fs-14 fw-700 c-accent-text min-h-44" onClick={onOpenJournal}>
                    {tr.home.allCount(list.length)}
                </button>
            </div>
            {list.slice(0, 3).map((entry) => (
                <div key={entry.at + entry.text} class="flex flex-col gap-2 py-8 px-0 bt-divider">
                    <span class="fw-700 overflow-hidden text-ellipsis nowrap">
                        {tr.home.quoted(entry.text)}
                    </span>
                    <span class="t-caption">
                        {timeAgo(entry.at)} · {entry.reason}
                    </span>
                </div>
            ))}
        </section>
    );
}
