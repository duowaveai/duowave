import { useEffect, useMemo, useState } from 'preact/hooks';

import { AccessCatalog, ApiError, RequestView, SessionSnapshot, Station, UnsupportedEntry, api, can, problemOf } from '../api';
import { CAPABILITY_TITLES } from '../capabilities';
import { clock, timeAgo } from '../time';
import { toneClass } from './dash-model';
import { KIND_WORDS, PLACE_WORDS, STEP_TITLES, answeredBy, seconds, statusOf } from './trip';

interface Props {
    debug: { sessions: SessionSnapshot[] | null; unsupported: UnsupportedEntry[]; error: ApiError | null };
    catalog: AccessCatalog | null;
    /** Запросы из живого канала: идущие и последние — поверх журнала. */
    live: RequestView[];
    stations: Station[] | null;
}

/** Сколько журнал запросов хранится: варианты сегмента, дни. */
const KEEP: Array<{ days: number; label: string }> = [
    { days: 0, label: 'Не хранить' },
    { days: 7, label: 'Неделю' },
    { days: 30, label: 'Месяц' },
    { days: 365, label: 'Год' },
];
const PAGE = 30;

const byStart = (a: RequestView, b: RequestView): number => b.startedAt.localeCompare(a.startedAt);

function debugHint(error: ApiError | null): string | null {
    if (!error) return null;
    if (error.status === 403) return problemOf(error);
    return 'Шлюз не отдал сессии — попробуйте позже.';
}

function SessionCard({ sessions }: { sessions: SessionSnapshot[] }) {
    const live = sessions.find((s) => !s.closedAt) ?? sessions[0];
    if (!live) return null;
    const device = live.device;
    return (
        <section class="card glass flex flex-col gap-10" aria-label="Колонка">
            <div class="flex items-baseline justify-between gap-8">
                <h2 class="t-title-xs m-0">
                    Колонка
                </h2>
                <span class="t-meta">
                    {live.closedAt ? `отключилась ${timeAgo(live.closedAt)}` : `подключена ${timeAgo(live.connectedAt)}`}
                </span>
            </div>
            <div class="flex flex-wrap gap-8">
                {device?.deviceModel && (
                    <span class="stat-chip">
                        <b>{device.deviceModel}</b>
                    </span>
                )}
                {device?.firmwareVersion && (
                    <span class="stat-chip">
                        <b>прошивка</b>
                        <span class="val">{device.firmwareVersion}</span>
                    </span>
                )}
                <span class="stat-chip">
                    <b>событий</b>
                    <span class="val">{live.events}</span>
                </span>
                {live.canSpeak && <span class="pill pill-own">умеет говорить</span>}
                {live.yandexConnected ? (
                    <span class="pill pill-yandex">Яндекс на связи</span>
                ) : (
                    <span class="pill pill-off">Яндекса нет</span>
                )}
            </div>
        </section>
    );
}

/** Полоска пути: шаги по длительности, цветом того, кто их делал; идущий шаг дышит. */
function TripBar({ request }: { request: RequestView }) {
    // Отметки плеера — не отрезки пути: полоска — только шаги со своей длительностью.
    const steps = request.steps.filter((step) => step.phase !== 'play');
    if (!steps.length) return null;
    return (
        <div class="req-bar" aria-hidden="true">
            {steps.map((step, index) => (
                <span
                    key={index}
                    class={`req-seg ${toneClass(step.by ?? 'off')}${step.ms === undefined ? ' going' : ''}`}
                    style={{ flexGrow: Math.max(step.ms ?? 400, 150) }}
                />
            ))}
        </div>
    );
}

/** Запрос строкой: время, слова, итог, полоска пути; по нажатию — шаги. И в журнале, и на главной. */
export function RequestRow({
    request,
    station,
    titles,
    open,
    onToggle,
}: {
    request: RequestView;
    station?: string;
    titles: Record<string, string>;
    open: boolean;
    onToggle: () => void;
}) {
    const status = statusOf(request);
    const by = answeredBy(request);
    const meta = [KIND_WORDS[request.kind], station, request.capability ? (titles[request.capability] ?? request.capability) : undefined]
        .filter(Boolean)
        .join(' · ');
    return (
        <li class="req-row">
            <button class="req-head" aria-expanded={open} onClick={onToggle}>
                <span class="req-time t-caption">{clock(request.startedAt)}</span>
                <span class="req-words">
                    <b>{request.text ? `«${request.text}»` : request.partial ? `«${request.partial}…»` : 'Без слов'}</b>
                    {request.answer && <span class="t-meta">{request.answer}</span>}
                </span>
                <span class={`pill ${status.cls}`}>{status.label}</span>
            </button>
            <TripBar request={request} />
            <div class="req-meta">
                {by && <span class={`pill pill-${by.tone === 'cloud' ? 'yandex' : by.tone}`}>{by.label}</span>}
                <span class="t-caption">{meta}</span>
            </div>
            {open && (
                <ol class="req-steps t-caption">
                    {request.steps.map((step, index) => (
                        <li key={index}>
                            <b>{STEP_TITLES[step.phase]}</b>
                            {/* Плеер — отметка того, что сказала колонка, не шаг со своей длительностью: когда это было. */}
                            {step.phase === 'play' ? ` на ${seconds(step.at)}` : ` ${step.ms !== undefined ? seconds(step.ms) : '…'}`}
                            {step.by ? ` · ${PLACE_WORDS[step.by]}` : ''}
                            {step.what ? ` · ${step.what}` : ''}
                        </li>
                    ))}
                    {request.error && <li class="req-error">{request.error}</li>}
                    {!request.steps.length && !request.error && <li>Шагов нет: запрос закончился, не начавшись.</li>}
                </ol>
            )}
        </li>
    );
}

/** Сколько хранить журнал: сегмент для того, кому можно менять настройки; укоротить — спросить. */
function KeepChoice({ days, onChanged }: { days: number; onChanged: (days: number) => void }) {
    const [pending, setPending] = useState<number | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const options = KEEP.some((option) => option.days === days) ? KEEP : [...KEEP, { days, label: `${days} дн.` }];
    const apply = async (next: number): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            const saved = await api.keepRequests(next);
            onChanged(saved.days);
            setPending(null);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const pick = (next: number): void => {
        if (next === days) return;
        // Короче, чем было: старое удалится сразу — сначала спросить.
        if (next === 0 || next < days) setPending(next);
        else void apply(next);
    };
    return (
        <div class="keep-choice">
            <span class="t-caption">Хранить журнал</span>
            <div
                role="radiogroup"
                aria-label="Сколько хранить журнал запросов"
                class={`look-seg glass keep-seg${options.length > 4 ? ' keep-seg-5' : ''}`}
            >
                {options.map((option) => (
                    <button
                        key={option.days}
                        role="radio"
                        aria-checked={option.days === (pending ?? days)}
                        disabled={busy}
                        onClick={() => pick(option.days)}
                    >
                        {option.label}
                    </button>
                ))}
            </div>
            {pending !== null && (
                <div class="keep-confirm" role="alert">
                    <span class="t-meta">
                        {pending === 0 ? 'Весь журнал запросов удалится сейчас.' : `Запросы старше ${pending} дн. удалятся сейчас.`}
                    </span>
                    <div class="flex gap-8">
                        <button class="pill pill-error dash-confirm-btn" disabled={busy} onClick={() => void apply(pending)}>
                            Удалить
                        </button>
                        <button class="pill pill-off dash-confirm-btn" disabled={busy} onClick={() => setPending(null)}>
                            Отмена
                        </button>
                    </div>
                </div>
            )}
            {error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {error}
                </span>
            )}
        </div>
    );
}

/** Журнал запросов: что спрашивали и что ответили, кто делал каждый шаг и сколько он шёл; живые — сверху. */
function RequestsCard({ live, stations, titles }: { live: RequestView[]; stations: Station[] | null; titles: Record<string, string> }) {
    const [history, setHistory] = useState<RequestView[]>([]);
    const [keepDays, setKeepDays] = useState<number | null>(null);
    const [more, setMore] = useState(false);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [open, setOpen] = useState<string | null>(null);

    const load = async (before?: string): Promise<void> => {
        setLoading(true);
        setError(null);
        try {
            const page = await api.requests({ limit: PAGE, before });
            setKeepDays(page.keepDays);
            setHistory((list) => (before ? [...list, ...page.requests] : page.requests));
            setMore(page.requests.length === PAGE);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setLoading(false);
        }
    };

    useEffect(() => {
        void load();
    }, []);

    // Живой канал свежее журнала: тот же запрос из него заменяет записанный.
    const rows = useMemo(() => {
        const byId = new Map<string, RequestView>();
        for (const request of history) byId.set(request.id, request);
        for (const request of live) byId.set(request.id, request);
        return [...byId.values()].sort(byStart);
    }, [history, live]);
    const names = new Map((stations ?? []).map((station) => [station.device, station.name]));
    const several = (stations ?? []).filter((station) => station.status === 'approved').length > 1;

    return (
        <section class="card glass px-16 pt-4 pb-12 flex flex-col" aria-labelledby="requests-h">
            <div class="card-head">
                <h2 id="requests-h" class="t-title-xs m-0">
                    Запросы
                </h2>
                <span class="t-meta">{keepDays === null ? '' : keepDays === 0 ? 'не хранятся' : `хранятся ${keepDays} дн.`}</span>
            </div>
            {error && (
                <p class="t-meta m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {rows.length === 0 && !loading ? (
                <p class="t-meta m-0 p-0 pt-8 pb-12">
                    Пока пусто: спросите колонку — запрос появится здесь, пока она отвечает.
                </p>
            ) : (
                <ol class="req-list">
                    {rows.map((request) => (
                        <RequestRow
                            key={request.id}
                            request={request}
                            station={several ? names.get(request.device) : undefined}
                            titles={titles}
                            open={open === request.id}
                            onToggle={() => setOpen(open === request.id ? null : request.id)}
                        />
                    ))}
                </ol>
            )}
            {more && (
                <button
                    class="pill pill-off self-center mt-8"
                    disabled={loading}
                    onClick={() => void load(history.at(-1)?.startedAt)}
                >
                    {loading ? 'Загружаю…' : 'Ещё'}
                </button>
            )}
            {keepDays !== null && can('settings.edit') && (
                <KeepChoice
                    days={keepDays}
                    onChanged={(days) => {
                        setKeepDays(days);
                        void load();
                    }}
                />
            )}
        </section>
    );
}

export function JournalScreen({ debug, catalog, live, stations }: Props) {
    const allowed = can('journal.view');
    const hint = debugHint(debug.error) ?? (allowed ? null : 'Что говорили колонке, видит тот, кому администратор дал право на журнал.');
    const titles: Record<string, string> = catalog
        ? Object.fromEntries(catalog.capabilities.map((capability) => [capability.id, capability.title]))
        : CAPABILITY_TITLES;

    return (
        <>
            {hint ? (
                <section class="card glass">
                    <p class="t-meta m-0">
                        {hint}
                    </p>
                </section>
            ) : (
                debug.sessions && <SessionCard sessions={debug.sessions} />
            )}

            {allowed && <RequestsCard live={live} stations={stations} titles={titles} />}

            <section class="card glass card-list card-list-tight" aria-labelledby="journal-h">
                <div class="card-head">
                    <h2 id="journal-h" class="t-title-xs m-0">
                        Пока не умеем
                    </h2>
                    <span class="t-meta">{debug.unsupported.length}</span>
                </div>
                {debug.unsupported.length === 0 ? (
                    <p class="t-meta m-0 p-0 pt-8 pb-12">
                        Пусто: всё, что просили, кто-то обслужил.
                    </p>
                ) : (
                    debug.unsupported.map((entry, index) => (
                        <div
                            key={`${entry.at}-${index}`}
                            class="flex flex-col gap-2 py-10 px-0 bt-divider"
                        >
                            <div class="flex items-center gap-8">
                                <span class="pill pill-off">{titles[entry.capability] ?? entry.capability}</span>
                                <span class="t-caption">{timeAgo(entry.at)}</span>
                            </div>
                            <span class="fw-700">«{entry.text}»</span>
                            <span class="t-caption">{entry.reason}</span>
                        </div>
                    ))
                )}
            </section>
        </>
    );
}
