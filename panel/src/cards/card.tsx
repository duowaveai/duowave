// Карточки приложений (docs/plans/bus.md, «Пульт»; детали — gateway/src/bus/cards.rs): функция приложения отдаёт
// данные — число с изменением, подпись, линию, столбики, плитки, картинку, часы, кольца с отсчётом, доли, строки,
// полоску, поле, кнопки, выбор, переключатель, — а пульт рисует их своими деталями. Кода приложения в пульте нет:
// карточка безопасна, легка и одинакова на столе, в телефоне и в чате. Нажатие зовёт функцию приложения (из его
// contributes.panel.calls) и получает новую карточку — или карточка перечитывается.

import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import { CardSpec, fetchBusPreview, fetchPluginFile, problemOf } from '../api';
import { usePluginVersion } from '../plugins/versions';
import { useApi } from '../sdk';
import { Action } from '../ui/cards';
import { Switch } from '../ui/switch';
import { LookLayers, lookOf, usePaint } from './look';
import './cards.css';
import { locale, tr } from '../i18n';

type Tone = 'up' | 'down' | 'flat';

export interface CardData {
    big?: string;
    delta?: { text: string; tone?: Tone };
    cap?: string;
    line?: { points: number[]; area?: boolean };
    bars?: { points: number[]; labels?: string[] };
    tiles?: Array<{ label: string; value: string; tap?: string; with?: unknown }>;
    /** Из своей папки (`name`) или из папки дома по связи приложения (`address: files/…`). */
    picture?: { name?: string; address?: string; caption?: string };
    clocks?: Array<{ label: string; offset: number }>;
    rings?: Array<{ label: string; endsAt: string; total?: number }>;
    shares?: { parts: Array<{ label: string; value: number }> };
    rows?: Array<{ text: string; value?: string; time?: string; delta?: { text: string; tone?: Tone }; mark?: string; tap?: string; with?: unknown }>;
    progress?: { value: number; text?: string };
    input?: { placeholder?: string; submit: string; max?: number };
    buttons?: Array<{ text: string; tap: string; with?: unknown; ink?: boolean }>;
    choice?: { options: Array<{ id: string; text: string }>; selected?: string; tap: string };
    toggle?: { text: string; on: boolean; tap: string };
    empty?: string;
    say?: string;
    /** Вид всего виджета (look.tsx): сцена, цвета, фото фоном, цвет текста. */
    look?: unknown;
}

const PARTS = ['big', 'delta', 'cap', 'line', 'bars', 'tiles', 'picture', 'clocks', 'rings', 'shares', 'rows', 'progress', 'input', 'buttons', 'choice', 'toggle', 'empty', 'say', 'look'];

/** Ответ функции — карточка: сама или в поле `card`. Не карточка — null. */
export function cardOf(answer: unknown): CardData | null {
    if (!answer || typeof answer !== 'object' || Array.isArray(answer)) return null;
    const inner = (answer as { card?: unknown }).card;
    const card = inner && typeof inner === 'object' && !Array.isArray(inner) ? inner : answer;
    return Object.keys(card as object).some((key) => PARTS.includes(key)) ? (card as CardData) : null;
}

/** Кнопка карточки нажата: функция и что ей передать. */
type Tap = (fn: string, input?: unknown) => void;

const numbers = (points: unknown): number[] => (Array.isArray(points) ? points.filter((point): point is number => typeof point === 'number' && Number.isFinite(point)) : []);

/** Число для подсказки: «1 234,5». */
const shown = (value: number): string => value.toLocaleString(locale(), { maximumFractionDigits: 2 });

function Line({ points, area }: { points: number[]; area?: boolean }) {
    const box = useRef<HTMLDivElement>(null);
    const [at, setAt] = useState<number | null>(null);
    if (points.length < 2) return null;
    const min = Math.min(...points);
    const max = Math.max(...points);
    const span = max - min || 1;
    const x = (index: number): number => (index / (points.length - 1)) * 100;
    const y = (value: number): number => 4 + (1 - (value - min) / span) * 92;
    const path = points.map((value, index) => `${index === 0 ? 'M' : 'L'}${x(index).toFixed(2)},${y(value).toFixed(2)}`).join(' ');
    const move = (event: PointerEvent): void => {
        const rect = box.current?.getBoundingClientRect();
        if (!rect || rect.width === 0) return;
        const share = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
        setAt(Math.round(share * (points.length - 1)));
    };
    return (
        <div ref={box} class="app-card-chart" onPointerMove={(event) => move(event as PointerEvent)} onPointerDown={(event) => move(event as PointerEvent)} onPointerLeave={() => setAt(null)}>
            <svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label={tr.cards.lineLabel(shown(points[0]), shown(points[points.length - 1]), shown(min), shown(max))}>
                {area && <path d={`${path} L100,100 L0,100 Z`} fill="var(--card-area)" stroke="none" />}
                <path d={path} fill="none" stroke="var(--card-line)" stroke-width="2" vector-effect="non-scaling-stroke" stroke-linejoin="round" stroke-linecap="round" />
                {at != null && <line x1={x(at)} x2={x(at)} y1="0" y2="100" stroke="var(--sub)" stroke-width="1" vector-effect="non-scaling-stroke" stroke-dasharray="3 3" />}
            </svg>
            {at != null && (
                <span class="tip" style={{ left: `${x(at)}%` }}>
                    {shown(points[at])}
                </span>
            )}
        </div>
    );
}

function Bars({ points, labels }: { points: number[]; labels?: string[] }) {
    const [at, setAt] = useState<number | null>(null);
    if (points.length === 0) return null;
    const max = Math.max(...points.map(Math.abs)) || 1;
    return (
        <div class="flex flex-col gap-4">
            <div class="app-card-chart" style={{ display: 'flex', alignItems: 'flex-end', gap: '2px' }} onPointerLeave={() => setAt(null)} role="img" aria-label={tr.cards.barsLabel(points.map(shown).join(', '))}>
                {points.map((value, index) => (
                    <span
                        key={index}
                        onPointerEnter={() => setAt(index)}
                        onPointerDown={() => setAt(index)}
                        style={{
                            flex: 1,
                            minWidth: 0,
                            height: `${Math.max(2, (Math.abs(value) / max) * 100)}%`,
                            background: 'var(--card-line)',
                            borderRadius: '4px 4px 0 0',
                            opacity: at == null || at === index ? 1 : 0.55,
                        }}
                    />
                ))}
                {at != null && (
                    <span class="tip" style={{ left: `${((at + 0.5) / points.length) * 100}%` }}>
                        {labels?.[at] ? `${labels[at]}: ` : ''}
                        {shown(points[at])}
                    </span>
                )}
            </div>
            {labels && labels.length > 0 && (
                <div class="app-card-bar-labels t-caption">
                    {points.map((_, index) => (
                        <span key={index}>{labels[index] ?? ''}</span>
                    ))}
                </div>
            )}
        </div>
    );
}

/** Часы: время по смещению от UTC, тикают раз в 20 с. */
function Clocks({ clocks }: { clocks: NonNullable<CardData['clocks']> }) {
    const [now, setNow] = useState(Date.now());
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 20_000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <div class="app-card-clocks">
            {clocks.map((clock, index) => {
                const there = new Date(now + (Number(clock.offset) || 0) * 60_000);
                const time = `${String(there.getUTCHours()).padStart(2, '0')}:${String(there.getUTCMinutes()).padStart(2, '0')}`;
                return (
                    <div key={index} class="app-card-clock">
                        <b>{time}</b>
                        <span class="t-caption">{clock.label}</span>
                    </div>
                );
            })}
        </div>
    );
}

const left = (ms: number): string => {
    if (ms <= 0) return tr.cards.ready;
    const total = Math.ceil(ms / 1000);
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const seconds = total % 60;
    return hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}` : `${minutes}:${String(seconds).padStart(2, '0')}`;
};

/** Кольца с отсчётом: тикают раз в секунду, пока хоть одно идёт. */
function Rings({ rings }: { rings: NonNullable<CardData['rings']> }) {
    const [now, setNow] = useState(Date.now());
    const ends = rings.map((ring) => Date.parse(ring.endsAt));
    const running = ends.some((end) => Number.isFinite(end) && end > now);
    useEffect(() => {
        if (!running) return;
        const timer = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
    }, [running]);
    const radius = 26;
    const around = 2 * Math.PI * radius;
    return (
        <div class="app-card-rings">
            {rings.map((ring, index) => {
                const remaining = Number.isFinite(ends[index]) ? ends[index] - now : 0;
                const share = ring.total && ring.total > 0 ? Math.min(1, Math.max(0, remaining / (ring.total * 1000))) : remaining > 0 ? 1 : 0;
                return (
                    <div key={index} class="app-card-ring">
                        <svg viewBox="0 0 64 64" aria-hidden="true">
                            <circle class="track" cx="32" cy="32" r={radius} fill="none" stroke-width="6" />
                            <circle class="left" cx="32" cy="32" r={radius} fill="none" stroke-width="6" stroke-linecap="round" stroke-dasharray={`${around}`} stroke-dashoffset={`${around * (1 - share)}`} transform="rotate(-90 32 32)" />
                        </svg>
                        <b>{left(remaining)}</b>
                        <span class="t-caption">{ring.label}</span>
                    </div>
                );
            })}
        </div>
    );
}

function Shares({ parts }: { parts: Array<{ label: string; value: number }> }) {
    const kept = parts.filter((part) => typeof part.value === 'number' && part.value > 0).slice(0, 6);
    const total = kept.reduce((sum, part) => sum + part.value, 0);
    if (total <= 0) return null;
    const percent = (value: number): string => `${Math.round((value / total) * 100)} %`;
    return (
        <div>
            <div class="app-card-shares-bar" role="img" aria-label={kept.map((part) => `${part.label} ${percent(part.value)}`).join(', ')}>
                {kept.map((part, index) => (
                    <span key={index} title={`${part.label}: ${shown(part.value)} (${percent(part.value)})`} style={{ flexGrow: part.value, background: `var(--card-share-${index + 1})` }} />
                ))}
            </div>
            <ul class="app-card-shares-legend">
                {kept.map((part, index) => (
                    <li key={index}>
                        <i style={{ background: `var(--card-share-${index + 1})` }} />
                        {part.label} · {percent(part.value)}
                    </li>
                ))}
            </ul>
        </div>
    );
}

function Picture({ app, name, address, caption }: { app: string; name?: string; address?: string; caption?: string }) {
    const [url, setUrl] = useState<string | null>(null);
    // Not there (an app that named a file it never downloaded) or not given: said, not an empty space.
    const [missing, setMissing] = useState(false);
    useEffect(() => {
        let made: string | null = null;
        let alive = true;
        setMissing(false);
        // A picture of the home folder comes made small, read as the app for whoever looks (its link holds).
        (address ? fetchBusPreview(app, address, 960) : fetchPluginFile(app, name ?? ''))
            .then((blob) => {
                if (!alive) return;
                made = URL.createObjectURL(blob);
                setUrl(made);
            })
            .catch(() => {
                if (!alive) return;
                setUrl(null);
                setMissing(true);
            });
        return () => {
            alive = false;
            if (made) URL.revokeObjectURL(made);
        };
    }, [app, name, address]);
    return (
        <figure class="app-card-picture m-0">
            {url && <img src={url} alt={caption ?? ''} />}
            {missing && (
                <p class="app-card-picture-missing t-caption m-0" role="status">
                    {tr.cards.pictureMissing(name)}
                </p>
            )}
            {caption && <figcaption class="t-caption">{caption}</figcaption>}
        </figure>
    );
}

/** Время строки: ISO — «14:05» сегодня или «12 нояб.», иначе как есть. */
function timeWord(time: string): string {
    const at = Date.parse(time);
    if (!Number.isFinite(at) || !/\d{4}-\d{2}-\d{2}/.test(time)) return time;
    const date = new Date(at);
    const today = new Date();
    return date.toDateString() === today.toDateString()
        ? date.toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' })
        : date.toLocaleDateString(locale(), { day: 'numeric', month: 'short' });
}

function Delta({ delta }: { delta: { text: string; tone?: Tone } }) {
    return (
        <span class="app-card-delta" data-tone={delta.tone ?? 'flat'}>
            {delta.tone === 'up' ? '▲ ' : delta.tone === 'down' ? '▼ ' : ''}
            {delta.text}
        </span>
    );
}

function Rows({ rows, tap, busy }: { rows: NonNullable<CardData['rows']>; tap: Tap; busy: boolean }) {
    return (
        <ul class="app-card-rows">
            {rows.map((row, index) => {
                const body: ComponentChildren = (
                    <>
                        {row.mark && <span class="mark" data-mark={row.mark} aria-hidden="true" />}
                        <span class="text">
                            {row.text}
                            {row.time && <span class="t-caption"> · {timeWord(row.time)}</span>}
                        </span>
                        {row.delta && <Delta delta={row.delta} />}
                        {row.value && <span class="value">{row.value}</span>}
                    </>
                );
                return (
                    <li key={index} class="app-card-row">
                        {row.tap ? (
                            <button type="button" class="app-card-row-tap" disabled={busy} onClick={() => tap(row.tap!, row.with ?? {})}>
                                {body}
                            </button>
                        ) : (
                            body
                        )}
                    </li>
                );
            })}
        </ul>
    );
}

function Field({ input, tap, busy }: { input: NonNullable<CardData['input']>; tap: Tap; busy: boolean }) {
    const [text, setText] = useState('');
    const send = (): void => {
        const value = text.trim();
        if (!value) return;
        tap(input.submit, { text: value });
        setText('');
    };
    return (
        <form
            class="app-card-input"
            onSubmit={(event) => {
                event.preventDefault();
                send();
            }}
        >
            <input
                class="field"
                value={text}
                placeholder={input.placeholder ?? ''}
                maxLength={Math.min(input.max ?? 500, 2000)}
                aria-label={input.placeholder ?? tr.cards.fieldLabel}
                disabled={busy}
                onInput={(event) => setText((event.target as HTMLInputElement).value)}
            />
            <Action tone="on" label={tr.cards.send} disabled={busy || !text.trim()} onClick={send} />
        </form>
    );
}

/** Карточка по её данным; `tap` — нажатие кнопки, строки, поля, выбора, переключателя. */
export function CardView({ app, card, tap, busy = false }: { app: string; card: CardData; tap: Tap; busy?: boolean }) {
    const lookKey = JSON.stringify(card.look ?? null);
    const look = useMemo(() => lookOf(card.look), [lookKey]);
    // В рамке стола вид красит весь виджет; без рамки (черновик, чат) — свою коробку.
    const framed = usePaint(app, look);
    const line = numbers(card.line?.points).slice(0, 200);
    const bars = numbers(card.bars?.points).slice(0, 60);
    const drawn =
        card.big || card.cap || line.length > 1 || bars.length > 0 || card.tiles?.length || card.picture || card.clocks?.length || card.rings?.length || card.shares || card.rows?.length || card.progress || card.input || card.buttons?.length || card.choice || card.toggle;
    const content = (
        <div class="app-card">
            {(card.big || card.delta) && (
                <div class="app-card-head">
                    {card.big && <span class="app-card-big">{card.big}</span>}
                    {card.delta && <Delta delta={card.delta} />}
                </div>
            )}
            {card.cap && <p class="t-meta m-0">{card.cap}</p>}
            {line.length > 1 && <Line points={line} area={card.line?.area} />}
            {bars.length > 0 && <Bars points={bars} labels={card.bars?.labels} />}
            {card.tiles && card.tiles.length > 0 && (
                <div class="app-card-tiles">
                    {card.tiles.slice(0, 8).map((tile, index) => (
                        <div key={index} class="app-card-tile">
                            <span class="t-caption">{tile.label}</span>
                            <b>{tile.value}</b>
                        </div>
                    ))}
                </div>
            )}
            {(card.picture?.name || card.picture?.address) && <Picture app={app} name={card.picture.name} address={card.picture.address} caption={card.picture.caption} />}
            {card.clocks && card.clocks.length > 0 && <Clocks clocks={card.clocks.slice(0, 6)} />}
            {card.rings && card.rings.length > 0 && <Rings rings={card.rings.slice(0, 6)} />}
            {card.shares && <Shares parts={card.shares.parts ?? []} />}
            {card.rows && card.rows.length > 0 && <Rows rows={card.rows.slice(0, 50)} tap={tap} busy={busy} />}
            {card.progress && (
                <div class="app-card-progress">
                    <div class="app-card-progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(Math.min(1, Math.max(0, card.progress.value)) * 100)}>
                        <span style={{ width: `${Math.min(1, Math.max(0, card.progress.value)) * 100}%` }} />
                    </div>
                    {card.progress.text && <span class="t-caption">{card.progress.text}</span>}
                </div>
            )}
            {card.choice && (
                <div class="app-card-actions" role="group">
                    {card.choice.options.slice(0, 8).map((option) => (
                        <Action key={option.id} on={card.choice!.selected === option.id} label={option.text} disabled={busy} onClick={() => tap(card.choice!.tap, { id: option.id })} />
                    ))}
                </div>
            )}
            {card.toggle && (
                <div class="app-card-toggle">
                    <span>{card.toggle.text}</span>
                    <Switch on={!!card.toggle.on} label={card.toggle.text} disabled={busy} onChange={(on) => tap(card.toggle!.tap, { on })} />
                </div>
            )}
            {card.input && <Field input={card.input} tap={tap} busy={busy} />}
            {card.buttons && card.buttons.length > 0 && (
                <div class="app-card-actions">
                    {card.buttons.slice(0, 4).map((button, index) => (
                        <Action key={index} tone={button.ink ? 'on' : 'off'} label={button.text} disabled={busy} onClick={() => tap(button.tap, button.with ?? {})} />
                    ))}
                </div>
            )}
            {!drawn && <p class="t-caption m-0">{card.empty ?? tr.cards.nothingYet}</p>}
        </div>
    );
    if (!look || framed) return content;
    return (
        <div class={`app-card-painted look-ink-${look.ink ?? 'light'}`}>
            <LookLayers app={app} look={look} />
            <div class="app-card-painted-body">{content}</div>
        </div>
    );
}

/** Не чаще: перечитывать карточку по своему таймеру. */
const MIN_REFRESH_S = 10;

/** Карточка приложения на столе: зовёт его функцию за данными, перечитывает с переменами его пакета и по таймеру. */
export function AppCard({ app, spec }: { app: string; spec: CardSpec }) {
    const { call } = useApi(app);
    const version = usePluginVersion(app);
    const [card, setCard] = useState<CardData | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [nonce, setNonce] = useState(0);
    useEffect(() => {
        let alive = true;
        call(spec.function, {})
            .then((answer) => {
                if (!alive) return;
                const next = cardOf(answer);
                setCard(next);
                setError(next ? null : tr.cards.notACard(spec.function));
            })
            .catch((e) => alive && setError(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [app, spec.function, version, nonce, call]);
    const every = spec.refreshS ? Math.max(MIN_REFRESH_S, spec.refreshS) : 0;
    useEffect(() => {
        if (!every) return;
        const timer = window.setInterval(() => {
            if (document.visibilityState === 'visible') setNonce((n) => n + 1);
        }, every * 1000);
        return () => window.clearInterval(timer);
    }, [every]);
    const tap = useMemo<Tap>(
        () => (fn, input = {}) => {
            setBusy(true);
            setError(null);
            call(fn, input)
                .then((answer) => {
                    const next = cardOf(answer);
                    if (next) setCard(next);
                    else setNonce((n) => n + 1);
                })
                .catch((e) => setError(problemOf(e)))
                .finally(() => setBusy(false));
        },
        [call],
    );
    if (!card && !error) return <p class="t-caption m-0">{tr.cards.loading}</p>;
    return (
        <div class="flex flex-col gap-8">
            {card && <CardView app={app} card={card} tap={tap} busy={busy} />}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}
