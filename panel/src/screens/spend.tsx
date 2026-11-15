// «Расход» (docs/plans/router-and-works.md, «Устройство: пульт», этап 6б) — «Персонаж» → «Мозг» → «Расход»,
// `#/character/brain/spend`. Место выбрал владелец 15.11.2026: внутри «Мозга», как в плане, хотя расход считает весь
// дом — разговор, мастерскую, память и приложения.
//
// Сверху — за сегодня, неделю или месяц: доллары и токены рядом, сколько сделок дома (бесплатно), сколько по подписке
// (как стоило бы по ключу) и сколько без цены. Ниже — по дням, по местам, по моделям (нажатие — цена модели: вписать
// руками или вернуть цену из снимка) и сами сделки с причиной. Всё из API этапа 1: `/api/v1/router/stats` одним
// запросом по дням, местам, провайдерам, моделям и способу оплаты, `/api/v1/router/deals`, `/api/v1/router/prices`.
// Сутки шлюз складывает по UTC — так их и показываем. Доллары — оценка (решение 12), рубли не считаем.
//
// Слова о сделке (место, причина, исход, деньги) и строка сделки живут здесь: их берут вкладка «Сделки» у работы
// (screens/work.tsx) и журнал запросов (screens/journal.tsx).

import { useState } from 'preact/hooks';

import { DealSums, ModelDeal, ModelPrice, PriceNow, PriceWrite, RouterStats, api, problemOf } from '../api';
import { IconChevronRight } from '../icons';
import { formatDate, formatNumber, tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { Sheet } from '../ui/sheet';
import { dollars, gatewayNow, placeWord, tokensShort } from './works';
import '../ui/spend.css';

// ─── Слова о сделке ─────────────────────────────────────────────────────────────────────────────────────────────

export function seatWord(seat: string): string {
    switch (seat) {
        case 'fast':
            return tr.router.seatFast;
        case 'smart':
            return tr.router.seatSmart;
        case 'backup':
            return tr.router.seatBackup;
        case 'workshop':
            return tr.router.seatWorkshop;
        case 'memory':
            return tr.router.seatMemory;
        case 'app':
            return tr.router.seatApp;
        default:
            return seat;
    }
}

export function reasonWord(reason: string): string {
    switch (reason) {
        case 'seat':
            return tr.router.reasonSeat;
        case 'backup':
            return tr.router.reasonBackup;
        case 'rule:command':
            return tr.router.reasonCommand;
        case 'rule:hard':
            return tr.router.reasonHard;
        case 'rule:short':
            return tr.router.reasonShort;
        default:
            return reason;
    }
}

export function outcomeWord(outcome: string): string | null {
    switch (outcome) {
        case 'failed':
            return tr.router.outcomeFailed;
        case 'cut':
            return tr.router.outcomeCut;
        case 'stopped':
            return tr.router.outcomeStopped;
        case 'limit':
            return tr.router.outcomeLimit;
        default:
            return null;
    }
}

/** Деньги словами: дома — «дома, бесплатно», по подписке — «≈ $…, по подписке», цены нет — «без цены». */
export interface Money {
    main: string;
    sub?: string;
    /** Обычные доллары по ключу — не слово. */
    plain?: boolean;
}

export function moneyOf(usdMicro: number | null | undefined, billing: string | undefined): Money {
    if (billing === 'local') return { main: tr.router.moneyHome, sub: tr.router.moneyFree };
    if (billing === 'plan') {
        return usdMicro === null || usdMicro === undefined ? { main: tr.router.moneyPlan } : { main: tr.router.moneyPlanUsd(dollars(usdMicro)), sub: tr.router.moneyPlan };
    }
    if (usdMicro === null || usdMicro === undefined) return { main: tr.router.noPrice };
    return { main: dollars(usdMicro), plain: true };
}

/** Деньги итога: один способ оплаты — его слова; смешанные — доллары и сколько сделок без цены. */
function moneyOfSums(sums: DealSums, billing: string | null): Money {
    if (billing === 'local') return moneyOf(0, 'local');
    const priced = sums.deals - sums.unpriced;
    const money = moneyOf(priced > 0 ? sums.usdMicro : null, billing ?? 'key');
    if (billing !== 'plan' && sums.unpriced > 0 && priced > 0) return { ...money, sub: tr.router.moneyUnpriced(sums.unpriced) };
    return money;
}

function MoneyCell({ money }: { money: Money }) {
    return (
        <span class={`sp-money${money.plain ? '' : ' is-word'}`}>
            <span class="wk-usd">{money.main}</span>
            {money.sub && <span class="wk-meta">{money.sub}</span>}
        </span>
    );
}

/**
 * Деньги нескольких сделок одной строкой: всё бесплатно — «бесплатно»; всё по подписке — «≈ $… по подписке»; иначе
 * доллары с пометкой «оценка».
 */
export function dealsMoney(deals: ModelDeal[]): string {
    const usd = deals.reduce((sum, deal) => sum + (deal.usdMicro ?? 0), 0);
    const priced = deals.every((deal) => deal.usdMicro !== null);
    if (usd === 0 && priced) return tr.router.moneyFree;
    if (deals.length > 0 && deals.every((deal) => deal.billing === 'plan')) return `${tr.router.moneyPlanUsd(dollars(usd))} ${tr.router.moneyPlan}`;
    return `${dollars(usd)} · ${tr.router.estimate}`;
}

const allTokens = (tokens: DealSums['tokens']): number => tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite;

/** Время шлюза (мс) по часам браузера: сегодня — «14:05», раньше — «15 нояб., 14:05». */
function dealTime(ms: number, skew: number, withDay: boolean): string {
    const at = ms - skew;
    return withDay ? formatDate(at, { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) : formatDate(at, { hour: '2-digit', minute: '2-digit' });
}

/** Сделка строкой: время, модель и место, причина и где думает, токены — и деньги справа. */
export function DealRow({ deal, skew, withDay = false, onWork }: { deal: ModelDeal; skew: number; withDay?: boolean; onWork?: (id: string) => void }) {
    const cache = deal.tokens.cacheRead + deal.tokens.cacheWrite;
    // Дома — это скажут деньги справа («дома, бесплатно»): место не повторяем.
    const notes = [reasonWord(deal.reason), deal.billing === 'local' ? null : placeWord(deal.place), outcomeWord(deal.outcome)].filter(Boolean);
    return (
        <li class="wk-deal">
            <span class={`w-cap wk-time${withDay ? ' sp-time-day' : ''}`}>{dealTime(deal.at, skew, withDay)}</span>
            <span class="wk-row-main">
                <span class="wk-row-title">
                    <b title={deal.provider}>{deal.model}</b>
                    <span class="wk-kind">{seatWord(deal.seat)}</span>
                </span>
                <span class="wk-meta">{notes.join(' · ')}</span>
                <span class="wk-meta">
                    {tr.router.dealTokens(tokensShort(deal.tokens.input), tokensShort(deal.tokens.output), cache ? tokensShort(cache) : '')}
                    {deal.work && onWork && (
                        <>
                            {' · '}
                            <button type="button" class="sp-link" onClick={() => onWork(deal.work as string)}>
                                {tr.router.dealOfWork(deal.work)}
                            </button>
                        </>
                    )}
                </span>
            </span>
            <MoneyCell money={moneyOf(deal.usdMicro, deal.billing)} />
        </li>
    );
}

// ─── Итоги ──────────────────────────────────────────────────────────────────────────────────────────────────────

type Period = 'day' | 'week' | 'month';
const PERIOD_DAYS: Record<Period, number> = { day: 1, week: 7, month: 30 };
const DAY_MS = 86_400_000;
/** Все строки сразу: по ним складываются дни, места и модели. */
const BY = ['day', 'seat', 'provider', 'model', 'billing'];
const DEALS_PAGE = 50;
const DEALS_MAX = 500;

/** День по UTC, как шлюз складывает сутки: «2026-11-15». */
function utcDay(ms: number): string {
    return new Date(ms).toISOString().slice(0, 10);
}

/** Первый и последний день периода по часам шлюза. */
function daysOf(period: Period, now: number): { from: string; to: string } {
    return { from: utcDay(now - (PERIOD_DAYS[period] - 1) * DAY_MS), to: utcDay(now) };
}

function emptySums(): DealSums {
    return { deals: 0, tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, rounds: 0, ms: 0, usdMicro: 0, unpriced: 0 };
}

function add(into: DealSums, row: DealSums): void {
    into.deals += row.deals;
    into.tokens.input += row.tokens.input;
    into.tokens.output += row.tokens.output;
    into.tokens.cacheRead += row.tokens.cacheRead;
    into.tokens.cacheWrite += row.tokens.cacheWrite;
    into.rounds += row.rounds;
    into.ms += row.ms;
    into.usdMicro += row.usdMicro;
    into.unpriced += row.unpriced;
}

interface Group {
    key: string;
    of: Record<string, string>;
    sums: DealSums;
    /** Способ оплаты, если он у всех строк один; разные — null. */
    billing: string | null;
}

/** Строки, сложенные по ключу; дороже и чаще — выше. */
function groupBy(rows: DealSums[], keyOf: (of: Record<string, string>) => string): Group[] {
    const groups = new Map<string, Group>();
    for (const row of rows) {
        const of = row.of ?? {};
        const key = keyOf(of);
        let group = groups.get(key);
        if (!group) {
            group = { key, of, sums: emptySums(), billing: of.billing ?? null };
            groups.set(key, group);
        } else if (group.billing !== of.billing) {
            group.billing = null;
        }
        add(group.sums, row);
    }
    return [...groups.values()].sort((a, b) => b.sums.usdMicro - a.sums.usdMicro || b.sums.deals - a.sums.deals);
}

/** Сколько сделок было дома, сколько долларов — по подписке. */
function shares(rows: DealSums[]): { home: number; planUsd: number; plan: number } {
    let home = 0;
    let planUsd = 0;
    let plan = 0;
    for (const row of rows) {
        if (row.of?.billing === 'local') home += row.deals;
        if (row.of?.billing === 'plan') {
            planUsd += row.usdMicro;
            plan += row.deals;
        }
    }
    return { home, planUsd, plan };
}

export function SpendPage() {
    const panel = usePanel();
    const skew = panel.feed.skew;
    const [period, setPeriod] = useState<Period>('day');
    const [stats, setStats] = useState<RouterStats | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [seat, setSeat] = useState<string | null>(null);
    const [pricing, setPricing] = useState<{ provider: string; model: string; billing: string | null } | null>(null);
    const days = daysOf(period, gatewayNow(skew));

    usePoll({
        version: panel.version,
        every: 30_000,
        deps: [period],
        read: (signal) =>
            api
                .routerStats(BY, days.from, days.to, signal)
                .then((fresh) => {
                    if (signal.aborted) return;
                    setStats(fresh);
                    setProblem(null);
                })
                .catch((e) => !signal.aborted && setProblem(problemOf(e))),
    });

    const pick = (next: Period) => {
        if (next === period) return;
        setStats(null);
        setPeriod(next);
    };

    return (
        <div class="wk-page">
            <Totals period={period} onPeriod={pick} stats={stats} problem={problem} />
            {stats && stats.total.deals > 0 && (
                <>
                    {period !== 'day' && <Days rows={stats.rows} />}
                    <Seats rows={stats.rows} picked={seat} onPick={setSeat} />
                    <Models rows={stats.rows} onPrice={(provider, model, billing) => setPricing({ provider, model, billing })} />
                </>
            )}
            {/* Пусто за период — сказано выше, второй раз не повторяем. */}
            {!(stats && stats.total.deals === 0) && <Deals from={days.from} withDay={period !== 'day'} seats={stats ? groupBy(stats.rows, (of) => of.seat ?? '').map((group) => group.key) : []} seat={seat} onSeat={setSeat} />}
            {pricing && <PriceSheet {...pricing} onClose={() => setPricing(null)} />}
        </div>
    );
}

const periods = (): Array<{ id: Period; label: string }> => [
    { id: 'day', label: tr.router.periodDay },
    { id: 'week', label: tr.router.periodWeek },
    { id: 'month', label: tr.router.periodMonth },
];

function Totals({ period, onPeriod, stats, problem }: { period: Period; onPeriod: (period: Period) => void; stats: RouterStats | null; problem: string | null }) {
    const total = stats?.total;
    const share = stats ? shares(stats.rows) : null;
    // Всё дома — «бесплатно», а не «$0.00».
    const allHome = !!total && !!share && total.deals > 0 && share.home === total.deals;
    const lines = share && total
        ? [
              share.home ? tr.router.spendHome(share.home) : '',
              share.plan ? tr.router.spendPlan(tr.router.moneyPlanUsd(dollars(share.planUsd))) : '',
              total.unpriced ? tr.router.spendUnpriced(total.unpriced) : '',
          ].filter(Boolean)
        : [];
    return (
        <section class="card glass card-list" aria-labelledby="sp-h">
            <div class="card-head sp-head">
                <h2 id="sp-h" class="t-title-xs m-0">
                    {tr.router.spendTitle}
                </h2>
                <div role="radiogroup" aria-label={tr.router.periodLabel} class="look-seg glass sp-periods">
                    {periods().map((entry) => (
                        <button key={entry.id} role="radio" aria-checked={entry.id === period} onClick={() => onPeriod(entry.id)}>
                            {entry.label}
                        </button>
                    ))}
                </div>
            </div>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {!total ? (
                !problem && <p class="w-cap m-0 wk-empty">{tr.router.reading}</p>
            ) : total.deals === 0 ? (
                <p class="w-cap m-0 wk-empty">{tr.router.spendNone}</p>
            ) : (
                <div class="sp-total">
                    <div class="sp-total-main">
                        <span class="sp-usd">{allHome ? tr.router.moneyFree : dollars(total.usdMicro)}</span>
                        {!allHome && <span class="w-cap">{tr.router.estimate}</span>}
                    </div>
                    <span class="sp-total-line">
                        {tr.router.spendTokens(tokensShort(allTokens(total.tokens)))} · {tr.router.seatDeals(total.deals)}
                    </span>
                    {lines.length > 0 && (
                        <ul class="sp-shares">
                            {lines.map((line) => (
                                <li key={line} class="wk-meta">
                                    {line}
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
            )}
            <p class="w-cap m-0 wk-under">{tr.router.spendAbout}</p>
        </section>
    );
}

/** День по UTC словами: «пн, 15 нояб.». */
function dayWord(day: string): string {
    return formatDate(Date.parse(`${day}T00:00:00Z`), { weekday: 'short', day: 'numeric', month: 'short', timeZone: 'UTC' });
}

/** По дням: полоска — доллары дня (а если долларов нет вовсе — токены), рядом то и другое числом. */
function Days({ rows }: { rows: DealSums[] }) {
    const days = groupBy(rows, (of) => of.day ?? '').sort((a, b) => b.key.localeCompare(a.key));
    const byUsd = days.some((day) => day.sums.usdMicro > 0);
    const measure = (sums: DealSums) => (byUsd ? sums.usdMicro : allTokens(sums.tokens));
    const most = Math.max(1, ...days.map((day) => measure(day.sums)));
    return (
        <section class="card glass card-list" aria-labelledby="sp-days-h">
            <div class="card-head">
                <h3 id="sp-days-h" class="t-title-xs m-0">
                    {tr.router.byDay}
                </h3>
                <span class="w-cap">{tr.router.byDayEmpty}</span>
            </div>
            <ul class="wk-rows">
                {days.map((day) => {
                    const money = moneyOfSums(day.sums, day.billing);
                    const tokens = tr.router.tokens(tokensShort(allTokens(day.sums.tokens)));
                    return (
                        <li key={day.key} class="sp-day" title={`${dayWord(day.key)}: ${money.main} · ${tokens}`}>
                            <span class="w-cap sp-day-name">{dayWord(day.key)}</span>
                            <span class="sp-bar" aria-hidden="true">
                                <span style={{ width: `${Math.max(2, (measure(day.sums) / most) * 100)}%` }} />
                            </span>
                            <span class="sp-day-value">
                                <span class="wk-usd">{money.main}</span>
                                <span class="wk-meta">{tokens}</span>
                            </span>
                        </li>
                    );
                })}
            </ul>
        </section>
    );
}

/** По местам: какое место сколько сделок, токенов и денег; нажатие — его сделки ниже. */
function Seats({ rows, picked, onPick }: { rows: DealSums[]; picked: string | null; onPick: (seat: string | null) => void }) {
    const seats = groupBy(rows, (of) => of.seat ?? '');
    return (
        <section class="card glass card-list" aria-labelledby="sp-seats-h">
            <div class="card-head">
                <h3 id="sp-seats-h" class="t-title-xs m-0">
                    {tr.router.bySeat}
                </h3>
            </div>
            <ul class="wk-rows">
                {seats.map((seat) => (
                    <li key={seat.key}>
                        <button type="button" class="wk-row-open sp-row" aria-pressed={picked === seat.key} onClick={() => onPick(picked === seat.key ? null : seat.key)}>
                            <span class="wk-row-main">
                                <span class="wk-row-title">
                                    <b>{seatWord(seat.key)}</b>
                                </span>
                                <span class="wk-meta">
                                    {tr.router.seatDeals(seat.sums.deals)} · {tr.router.tokens(tokensShort(allTokens(seat.sums.tokens)))}
                                </span>
                            </span>
                            <MoneyCell money={moneyOfSums(seat.sums, seat.billing)} />
                        </button>
                    </li>
                ))}
            </ul>
        </section>
    );
}

function billingWord(billing: string | null): string | null {
    if (billing === 'local') return tr.router.moneyHome;
    if (billing === 'plan') return tr.router.moneyPlan;
    return null;
}

/** По моделям: провайдер, как платим, токены, деньги; нажатие — цена модели. */
function Models({ rows, onPrice }: { rows: DealSums[]; onPrice: (provider: string, model: string, billing: string | null) => void }) {
    const models = groupBy(rows, (of) => `${of.provider ?? ''}\u0000${of.model ?? ''}`);
    return (
        <section class="card glass card-list" aria-labelledby="sp-models-h">
            <div class="card-head">
                <h3 id="sp-models-h" class="t-title-xs m-0">
                    {tr.router.byModel}
                </h3>
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.byModelAbout}</p>
            <ul class="wk-rows">
                {models.map((model) => {
                    const provider = model.of.provider ?? '';
                    const name = model.of.model ?? '';
                    const meta = [provider, billingWord(model.billing), tr.router.tokens(tokensShort(allTokens(model.sums.tokens)))].filter(Boolean);
                    return (
                        <li key={model.key}>
                            <button
                                type="button"
                                class="wk-row-open sp-row"
                                aria-label={tr.router.priceOpen(name)}
                                onClick={() => onPrice(provider, name, model.billing)}
                            >
                                <span class="wk-row-main">
                                    <span class="wk-row-title">
                                        <b title={name}>{name}</b>
                                    </span>
                                    <span class="wk-meta">{meta.join(' · ')}</span>
                                </span>
                                <MoneyCell money={moneyOfSums(model.sums, model.billing)} />
                                <IconChevronRight size={16} />
                            </button>
                        </li>
                    );
                })}
            </ul>
        </section>
    );
}

/** Сделки периода, новые первыми; можно оставить одно место. */
function Deals({ from, withDay, seats, seat, onSeat }: { from: string; withDay: boolean; seats: string[]; seat: string | null; onSeat: (seat: string | null) => void }) {
    const panel = usePanel();
    const [deals, setDeals] = useState<ModelDeal[] | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [limit, setLimit] = useState(DEALS_PAGE);
    usePoll({
        version: panel.version,
        every: 30_000,
        deps: [from, seat, limit],
        read: (signal) =>
            api
                .routerDeals({ from, seat: seat ?? undefined, limit }, signal)
                .then((answer) => {
                    if (signal.aborted) return;
                    setDeals(answer.deals);
                    setProblem(null);
                })
                .catch((e) => !signal.aborted && setProblem(problemOf(e))),
    });
    const choose = (next: string | null) => {
        setLimit(DEALS_PAGE);
        onSeat(next);
    };
    const full = !!deals && deals.length >= limit;
    return (
        <section class="card glass card-list" aria-labelledby="sp-deals-h">
            <div class="card-head">
                <h3 id="sp-deals-h" class="t-title-xs m-0">
                    {tr.router.dealsTitle}
                </h3>
                {full && <span class="w-cap">{tr.router.dealsLast(deals.length)}</span>}
            </div>
            {seats.length > 1 && (
                <div class="sp-chips" role="group" aria-label={tr.router.dealsSeatLabel}>
                    <button type="button" class="sp-chip" aria-pressed={seat === null} onClick={() => choose(null)}>
                        {tr.router.dealsAll}
                    </button>
                    {seats.map((entry) => (
                        <button key={entry} type="button" class="sp-chip" aria-pressed={seat === entry} onClick={() => choose(seat === entry ? null : entry)}>
                            {seatWord(entry)}
                        </button>
                    ))}
                </div>
            )}
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {!deals ? (
                !problem && <p class="w-cap m-0 wk-empty">{tr.router.reading}</p>
            ) : deals.length === 0 ? (
                <p class="w-cap m-0 wk-empty">{tr.router.spendNone}</p>
            ) : (
                <ul class="wk-rows">
                    {deals.map((deal) => (
                        <DealRow key={deal.id} deal={deal} skew={panel.feed.skew} withDay={withDay} onWork={(id) => panel.go('works', id)} />
                    ))}
                </ul>
            )}
            {full && limit < DEALS_MAX && (
                <button type="button" class="pill pill-off wk-more" onClick={() => setLimit(Math.min(DEALS_MAX, limit + DEALS_PAGE))}>
                    {tr.router.dealsMore}
                </button>
            )}
        </section>
    );
}

// ─── Цена модели ────────────────────────────────────────────────────────────────────────────────────────────────

/** Доллары за миллион токенов: «$3», «$0.075». */
function perMillion(value: number | null | undefined): string {
    if (value === null || value === undefined) return '—';
    return `$${formatNumber(value, { maximumFractionDigits: 4 })}`;
}

function priceLine(price: ModelPrice): string {
    const line = tr.router.priceLine(perMillion(price.input), perMillion(price.output));
    if (price.cacheRead === undefined && price.cacheWrite === undefined) return line;
    return `${line} · ${tr.router.priceCache(perMillion(price.cacheRead ?? price.input), perMillion(price.cacheWrite ?? price.input))}`;
}

function sourceWord(from: string): string {
    switch (from) {
        case 'manual':
            return tr.router.priceFromManual;
        case 'provider':
            return tr.router.priceFromProvider;
        case 'snapshot':
            return tr.router.priceFromSnapshot;
        case 'local':
            return tr.router.priceFromLocal;
        default:
            return from;
    }
}

const text = (value: number | null | undefined): string => (value === null || value === undefined ? '' : String(value));

/** Число из поля: «0,3» и «0.3»; пусто — undefined, не число — NaN. */
function read(value: string): number | undefined {
    const trimmed = value.trim().replace(',', '.');
    if (!trimmed) return undefined;
    const number = Number(trimmed);
    return Number.isFinite(number) && number >= 0 && number <= 100_000 ? number : Number.NaN;
}

/**
 * Цена модели: какая сейчас и откуда; вписать свою — следующие сделки считаются по ней; вписанную можно убрать —
 * вернётся цена провайдера или снимка. У модели дома цены нет — платить не за что.
 */
function PriceSheet({ provider, model, billing, onClose }: { provider: string; model: string; billing: string | null; onClose: () => void }) {
    const [now, setNow] = useState<PriceNow | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [notice, setNotice] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [fields, setFields] = useState({ input: '', output: '', cacheRead: '', cacheWrite: '' });
    const home = billing === 'local';

    const fill = (fresh: PriceNow) => {
        setNow(fresh);
        const price = fresh.price;
        setFields({ input: text(price?.input), output: text(price?.output), cacheRead: text(price?.cacheRead), cacheWrite: text(price?.cacheWrite) });
    };

    usePoll({
        deps: [provider, model],
        on: !home,
        read: (signal) =>
            api
                .modelPrice(provider, model)
                .then((fresh) => !signal.aborted && fill(fresh))
                .catch((e) => !signal.aborted && setProblem(problemOf(e))),
    });

    const save = async (price: PriceWrite | null) => {
        setBusy(true);
        setProblem(null);
        setNotice(null);
        try {
            fill(await api.writePrice(provider, model, price));
            setNotice(price ? tr.router.priceSaved : tr.router.priceReturned);
        } catch (e) {
            setProblem(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const submit = (event: Event) => {
        event.preventDefault();
        const input = read(fields.input);
        const output = read(fields.output);
        const cacheRead = read(fields.cacheRead);
        const cacheWrite = read(fields.cacheWrite);
        if (input === undefined || output === undefined || [input, output, cacheRead, cacheWrite].some((value) => Number.isNaN(value))) {
            setProblem(tr.router.priceNeedBoth);
            return;
        }
        void save({ input, output, ...(cacheRead === undefined ? {} : { cacheRead }), ...(cacheWrite === undefined ? {} : { cacheWrite }) });
    };

    const backLabel = !now?.unwritten ? tr.router.priceBackNone : now.unwritten.from === 'provider' ? tr.router.priceBackProvider : tr.router.priceBackSnapshot;
    const field = (name: keyof typeof fields, label: string, hint?: string) => (
        <label class="sp-field">
            <span class="w-cap">{label}</span>
            <input
                class="field"
                inputMode="decimal"
                autoComplete="off"
                placeholder={hint ?? ''}
                value={fields[name]}
                disabled={busy}
                onInput={(event) => {
                    const value = (event.target as HTMLInputElement).value;
                    setFields((all) => ({ ...all, [name]: value }));
                }}
            />
        </label>
    );

    return (
        <Sheet title={model} about={home ? provider : tr.router.priceAbout(provider)} onClose={onClose}>
            {home ? (
                <p class="t-meta m-0">{tr.router.priceHome}</p>
            ) : !now ? (
                <p class="t-meta m-0">{problem ?? tr.router.priceReading}</p>
            ) : (
                <form class="sp-price" onSubmit={submit}>
                    <div class="sp-price-now">
                        <span class="w-cap">{tr.router.priceNow}</span>
                        {now.price ? (
                            <>
                                <b>{priceLine(now.price)}</b>
                                <span class="w-cap">{sourceWord(now.price.from)}</span>
                            </>
                        ) : (
                            <span class="t-meta">{tr.router.priceNone}</span>
                        )}
                        {(now.plan || billing === 'plan') && <span class="w-cap">{tr.router.pricePlan}</span>}
                    </div>
                    <div class="sp-fields">
                        {field('input', tr.router.priceInput)}
                        {field('output', tr.router.priceOutput)}
                        {field('cacheRead', tr.router.priceCacheRead, tr.router.priceCacheEmpty)}
                        {field('cacheWrite', tr.router.priceCacheWrite, tr.router.priceCacheEmpty)}
                    </div>
                    <p class="w-cap m-0">{tr.router.priceNext}</p>
                    {/* Что будет без вписанной — до кнопки, а не после. */}
                    {now.written && <p class="w-cap m-0">{now.unwritten ? tr.router.priceBackWill(priceLine(now.unwritten)) : tr.router.priceBackWillNone}</p>}
                    {problem && (
                        <p class="w-cap m-0 c-dot-offline" role="alert">
                            {problem}
                        </p>
                    )}
                    {notice && (
                        <p class="w-cap m-0" role="status">
                            {notice}
                        </p>
                    )}
                    <div class="flex gap-8 flex-wrap">
                        <button type="submit" class="pill pill-own dash-confirm-btn" disabled={busy}>
                            {busy ? tr.router.priceSaving : tr.router.priceSave}
                        </button>
                        {now.written && (
                            <button type="button" class="pill pill-off dash-confirm-btn" disabled={busy} onClick={() => void save(null)}>
                                {backLabel}
                            </button>
                        )}
                    </div>
                </form>
            )}
        </Sheet>
    );
}

// ─── Строка в «Мозге» ───────────────────────────────────────────────────────────────────────────────────────────

/** Строка «Расход» в «Мозге»: сколько сегодня — и вглубь, на страницу. */
export function SpendRow({ onOpen }: { onOpen: () => void }) {
    const panel = usePanel();
    const [today, setToday] = useState<RouterStats | null>(null);
    usePoll({
        version: panel.version,
        every: 60_000,
        read: (signal) => {
            const day = utcDay(gatewayNow(panel.feed.skew));
            return api
                .routerStats(['billing'], day, day, signal)
                .then((fresh) => !signal.aborted && setToday(fresh))
                .catch(() => undefined);
        },
    });
    // Всё дома — «дома», всё по подписке — «≈ $…»; иначе доллары.
    const billing = today ? (groupBy(today.rows, () => '')[0]?.billing ?? null) : null;
    const sub =
        today === null
            ? tr.router.spendRowSub
            : today.total.deals === 0
              ? tr.router.spendTodayNone
              : tr.router.spendToday(moneyOfSums(today.total, billing).main, tr.router.tokens(tokensShort(allTokens(today.total.tokens))));
    return (
        <section class="card glass card-list" aria-label={tr.router.spendTitle}>
            <button type="button" class="wk-row-open sp-row" onClick={onOpen}>
                <span class="wk-row-main">
                    <span class="wk-row-title">
                        <b>{tr.router.spendTitle}</b>
                    </span>
                    <span class="wk-meta">{sub}</span>
                </span>
                <IconChevronRight size={16} />
            </button>
        </section>
    );
}
