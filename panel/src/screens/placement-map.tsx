import { useEffect, useRef, useState } from 'preact/hooks';

import { BRANDS, type BrandId } from '../brand-icons';
import type { Placement } from '../availability';
import type { Host, Solved, Who } from '../truth';
import { DuoWaveMark } from '../ui/duowave';

/**
 * Схема «Где работает DuoWave» — по эскизам владельца (ноябрь 2026): две зоны, «Дома» и «Облако» через интернет, и
 * топология под каждое размещение. Шлюз — хаб: зелёное остаётся дома, оранжевое уходит в интернет; точка бежит по
 * живым связям из того же расчёта, что и строка главного экрана (truth.ts). Имя модели не хардкодим: выбранная —
 * её имя, никакой — «ChatGPT, DeepSeek…».
 */

export function NodeIcon({ host }: { host: Host }) {
    const stroke = { fill: 'none', stroke: 'currentColor', 'stroke-width': 1.8, 'stroke-linecap': 'round' as const, 'stroke-linejoin': 'round' as const };
    switch (host) {
        case 'station':
            return (
                <g {...stroke}>
                    <rect x="5" y="3" width="14" height="18" rx="7" />
                    <path d="M8.5 15h7" />
                    <circle cx="12" cy="9" r="1.2" fill="currentColor" />
                </g>
            );
        case 'pc':
            return (
                <g {...stroke}>
                    <rect x="3" y="4" width="18" height="12" rx="2.5" />
                    <path d="M9 20h6M12 16v4" />
                </g>
            );
        case 'cloud':
            return (
                <g {...stroke}>
                    <path d="M7 18h10.5a4 4 0 0 0 .4-8A6 6 0 0 0 6.4 9.3 4.4 4.4 0 0 0 7 18z" />
                </g>
            );
        default:
            return (
                <g {...stroke}>
                    <circle cx="12" cy="12" r="8.5" />
                    <path d="M9 12.5c1.4 2 4.6 2 6 0" />
                </g>
            );
    }
}

export function Gear({ size = 16 }: { size?: number }) {
    return (
        <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
            <path
                fill="currentColor"
                d="M10.3 3.2h3.4l.5 2.6c.6.2 1.2.5 1.7.9l2.5-.9 1.7 2.9-2 1.7c.1.6.1 1.3 0 1.9l2 1.7-1.7 2.9-2.5-.9c-.5.4-1.1.7-1.7.9l-.5 2.6h-3.4l-.5-2.6c-.6-.2-1.2-.5-1.7-.9l-2.5.9-1.7-2.9 2-1.7a6 6 0 0 1 0-1.9l-2-1.7 1.7-2.9 2.5.9c.5-.4 1.1-.7 1.7-.9zM12 9.8a2.9 2.9 0 1 0 0 5.8 2.9 2.9 0 0 0 0-5.8z"
            />
        </svg>
    );
}

const HOP_MS = 1300;
const CLOUD_X = 292;
/** Все карточки одной высоты: строка значков умений сверху зарезервирована всегда — карточки не меняют размер. */
const CARD_H = 56;

/**
 * Карточка схемы. Шлюзом карточка не называется — где шлюз, показывает значок DuoWave, который переезжает с
 * карточки на карточку. Умения ПК (распознавание, синтез, подключения) — мелкие значки под подписью с подсказкой
 * при наведении, отдельных карточек им нет: карточек всего четыре — Колонка, ПК, Модели и агенты, Облачный сервис.
 */
interface Card {
    id: string;
    x: number;
    y: number;
    w: number;
    title: string;
    sub?: string;
    home: boolean;
    /** Есть, но выключено — «ПК» без компьютера: пунктирный контур и полупрозрачность. */
    dim?: boolean;
    /** Мелкие значки умений под подписью. */
    caps?: Array<'rec' | 'tts' | 'conn'>;
    /** Серые знаки известных моделей — на карточке «Модели и агенты»; выбранная зеленеет. */
    brands?: BrandId[];
    /** Значок Алисы — на карточке «Облачный сервис»; янтарный, когда сервис в деле. */
    alice?: boolean;
}

/** Связь: местная (зелёная) или через интернет (оранжевая, пунктир); правило — когда она жива. */
interface Link {
    a: string;
    b: string;
    kind: 'local' | 'net';
    lit: 'mic' | 'helps' | 'models' | 'service';
}

/**
 * Топология под размещение. Все четыре карточки стоят на одних местах при любом размещении — при переключении
 * сегментов меняются только связи, карточки не прыгают: Колонка сверху, ПК под ней, Модели и Облачный сервис —
 * в облаке справа.
 */
function layoutOf(placement: Placement, brainName?: string): { cards: Card[]; links: Link[]; homeSub: string; caption: string } {
    const models: Card = {
        id: 'models',
        x: CLOUD_X,
        y: 88,
        w: 120,
        title: 'Модели и агенты',
        sub: brainName ?? 'ChatGPT, DeepSeek…',
        home: false,
        brands: ['openai', 'claude', 'gemini', 'deepseek'],
    };
    const service: Card = { id: 'service', x: CLOUD_X, y: 176, w: 120, title: 'Облачный сервис', sub: 'Алиса', home: false, alice: true };
    const column = (caps?: Card['caps']): Card => ({ id: 'column', x: 111, y: 88, w: 150, title: 'Колонка', sub: 'слушает и говорит', home: true, caps });
    const pcCard = (extra: Partial<Card>): Card => ({ id: 'pc', x: 111, y: 176, w: 150, title: 'ПК', home: true, ...extra });
    switch (placement) {
        case 'pc':
            return {
                cards: [column(), pcCard({ caps: ['rec', 'tts', 'conn'] }), models, service],
                links: [
                    { a: 'column', b: 'pc', kind: 'local', lit: 'mic' },
                    { a: 'pc', b: 'models', kind: 'net', lit: 'models' },
                    { a: 'pc', b: 'service', kind: 'net', lit: 'service' },
                ],
                homeSub: 'колонка и компьютер в вашей сети',
                caption: 'Распознавание и синтез — дома, на ПК. Наружу — только текст запроса модели и облачному сервису.',
            };
        case 'station':
            return {
                cards: [column(['conn']), pcCard({ sub: 'можно выключить', dim: true }), models, service],
                links: [
                    { a: 'column', b: 'models', kind: 'net', lit: 'models' },
                    { a: 'column', b: 'service', kind: 'net', lit: 'service' },
                ],
                homeSub: 'только колонка — компьютер можно выключить',
                caption: 'Шлюз на самой колонке. Слышит и говорит Алиса или облако на выбор, думает облачная модель.',
            };
        default:
            return {
                cards: [column(['conn']), pcCard({ sub: 'помощник', caps: ['rec', 'tts'] }), models, service],
                links: [
                    { a: 'column', b: 'pc', kind: 'local', lit: 'helps' },
                    { a: 'column', b: 'models', kind: 'net', lit: 'models' },
                    { a: 'column', b: 'service', kind: 'net', lit: 'service' },
                ],
                homeSub: 'колонка и компьютер вместе',
                caption: 'Пока ПК на связи — слух и голос на нём. ПК уснул — их берёт облако или Алиса, колонка не молчит.',
            };
    }
}

/** Выбранная модель → знак бренда, который зеленеет на карточке «Модели и агенты». */
const BRAND_OF_NAME: Record<string, BrandId> = {
    Claude: 'claude',
    ChatGPT: 'openai',
    GPT: 'openai',
    Gemini: 'gemini',
    Mistral: 'mistral',
    DeepSeek: 'deepseek',
};

/** Знак бренда серым, как значки умений; выбранная модель — зелёная. */
function BrandGlyph({ id, on }: { id: BrandId; on: boolean }) {
    const brand = BRANDS[id];
    return (
        <g class={`cap${on ? ' on' : ''}`}>
            <title>{brand.title}</title>
            <svg width="13" height="13" viewBox={brand.viewBox ?? '0 0 24 24'} fill="currentColor" aria-hidden="true">
                <path d={brand.path} fill-rule={brand.fillRule} />
            </svg>
        </g>
    );
}

/** Значок Алисы — смайлик, как на остальных схемах пульта; в деле — янтарный. */
function AliceGlyph({ on }: { on: boolean }) {
    return (
        <g class={`cap alice${on ? ' on' : ''}`}>
            <title>Алиса</title>
            <svg width="13" height="13" viewBox="0 0 24 24" aria-hidden="true">
                <g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="8.5" />
                    <path d="M9 12.5c1.4 2 4.6 2 6 0" />
                </g>
            </svg>
        </g>
    );
}
const CAP_TITLES: Record<NonNullable<Card['caps']>[number], string> = { rec: 'Распознавание речи', tts: 'Синтез речи', conn: 'Подключения' };
const CAP_ON: Record<NonNullable<Card['caps']>[number], (solved: Solved) => boolean> = {
    rec: (solved) => solved.ear.host === 'pc',
    tts: (solved) => solved.voice.host === 'pc',
    conn: (solved) => !solved.silent,
};

function CapIcon({ cap }: { cap: NonNullable<Card['caps']>[number] }) {
    const stroke = { fill: 'none', stroke: 'currentColor', 'stroke-width': 1.8, 'stroke-linecap': 'round' as const, 'stroke-linejoin': 'round' as const };
    return (
        <svg width="13" height="13" viewBox="0 0 24 24" aria-hidden="true">
            {cap === 'rec' && (
                <g {...stroke}>
                    <rect x="9" y="3" width="6" height="11" rx="3" />
                    <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
                </g>
            )}
            {cap === 'tts' && (
                <g {...stroke}>
                    <path d="M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" />
                </g>
            )}
            {cap === 'conn' && (
                <g {...stroke}>
                    <path d="M9 3v5M15 3v5M7 8h10v4a5 5 0 0 1-10 0zM12 17v4" />
                </g>
            )}
        </svg>
    );
}

/**
 * Сама схема: зоны «Дома» и «Облако», четыре карточки, связи с точками цвета («остаётся дома» / «через интернет»);
 * значок DuoWave стоит на карточке, где работает шлюз, и по живым связям бежит точка. Что живо — из того же расчёта
 * правды, что и строка главного экрана (solved).
 */
export function PlacementMap({
    solved,
    placement,
    settingsAt,
    brainName,
}: {
    solved: Solved;
    placement: Placement;
    /** Где главные настройки: значок шестерёнки встаёт на ту карточку. */
    settingsAt: Host;
    /** Выбранная модель, если есть: пишем её имя, а не «ChatGPT, DeepSeek…». */
    brainName?: string;
}) {
    const packet = useRef<SVGCircleElement>(null);
    const reduced = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
    const { cards, links, homeSub, caption } = layoutOf(placement, brainName);
    const at = new Map(cards.map((card) => [card.id, card]));

    // Живость связей: голос колонки, помощь ПК, облачная модель, облачный сервис.
    const yandex = [solved.ear.host, solved.brain.host, solved.voice.host].includes('yandex');
    const pcWorks = solved.ear.host === 'pc' || solved.voice.host === 'pc';
    const alive: Record<Link['lit'], boolean> = {
        mic: !solved.silent,
        helps: !solved.silent && pcWorks,
        models: !solved.silent && solved.brain.host === 'cloud',
        service: !solved.silent && yandex,
    };
    const idOf = (link: Link): string => [link.a, link.b].sort().join('>');
    const litIds = new Set(links.filter((link) => alive[link.lit]).map(idOf));

    // Карточка горит, если до неё доходит живая связь.
    const cardLit = (card: Card): 'lit-own' | 'lit-out' | 'dim' | '' => {
        if (card.dim) return 'dim';
        const mine = links.filter((link) => link.a === card.id || link.b === card.id);
        return mine.some((link) => litIds.has(idOf(link))) ? (card.home ? 'lit-own' : 'lit-out') : '';
    };

    // Точка бежит по всем связям туда-обратно по кругу: схема — не только как работает сейчас, но и как может.
    // Живые связи точка проходит цветом работы (зелёный дома, оранжевый наружу), возможные — приглушённым серым.
    const run = links;
    const signature = run.map(idOf).join(' ');
    useEffect(() => {
        if (reduced || !run.length) return;
        let frame = 0;
        let index = 0;
        let started = performance.now();
        const tick = (now: number) => {
            let t = (now - started) / (HOP_MS * 2);
            if (t >= 1) {
                index = (index + 1) % run.length;
                started = now;
                t = 0;
            }
            const link = run[index];
            const dot = packet.current;
            const from = link && at.get(link.a);
            const to = link && at.get(link.b);
            if (dot && from && to) {
                // Туда и обратно за один проход по связи.
                const tt = t < 0.5 ? t * 2 : 2 - t * 2;
                const k = tt < 0.5 ? 2 * tt * tt : 1 - Math.pow(-2 * tt + 2, 2) / 2;
                dot.setAttribute('cx', String(from.x + (to.x - from.x) * k));
                dot.setAttribute('cy', String(from.y + (to.y - from.y) * k));
                const on = alive[link.lit];
                dot.style.fill = on ? (link.kind === 'local' ? 'var(--bar-own)' : 'var(--bar-yandex)') : 'var(--sub)';
                dot.style.opacity = on ? '1' : '0.55';
            }
            frame = requestAnimationFrame(tick);
        };
        frame = requestAnimationFrame(tick);
        return () => cancelAnimationFrame(frame);
    }, [signature, reduced]);

    const badge = (id: string, dx: number, dy: number) => {
        const card = at.get(id);
        return { transform: `translate(${(card?.x ?? 0) + dx}px, ${(card?.y ?? 0) + dy}px)` };
    };
    // Шлюз — там, где он работает: на ПК в «На компьютере», на колонке в остальных. Настройки — где их хранят.
    const gatewayCard = placement === 'pc' ? 'pc' : 'column';
    const gearCard = settingsAt === 'pc' ? 'pc' : 'column';
    const gateway = at.get(gatewayCard);
    const text = solved.silent ? 'Голосу некуда идти: DuoWave на ПК, а ПК спит.' : caption;

    return (
        <div class="map-card">
            <svg class="place-map" viewBox="0 0 360 244" role="img" aria-label={`Схема: куда идёт голос. ${text}`}>
                {/* Зоны: дом и облако через интернет; подписи зон — выше карточек, ничего не перекрывают. */}
                <rect x="4" y="22" width="214" height="200" rx="20" class="zone home" />
                <rect x="228" y="22" width="128" height="200" rx="20" class="zone cloud" />
                <text class="zone-title" x="16" y="42">
                    Дома
                </text>
                <text class="zone-sub" x="16" y="55">
                    {homeSub}
                </text>
                <text class="zone-title" x="292" y="42" text-anchor="middle">
                    Облако
                </text>
                <text class="zone-sub" x="292" y="55" text-anchor="middle">
                    через интернет
                </text>

                {/* Связи: зелёные — дома, оранжевые пунктирные — наружу; точка цвета — на середине. */}
                {links.map((link) => {
                    const from = at.get(link.a);
                    const to = at.get(link.b);
                    if (!from || !to) return null;
                    const id = idOf(link);
                    const on = litIds.has(id);
                    return (
                        <g key={id}>
                            <line
                                class={`edge ${link.kind}${on ? ` on ${link.kind === 'local' ? 'own' : 'yandex'}` : ''}`}
                                x1={from.x}
                                y1={from.y}
                                x2={to.x}
                                y2={to.y}
                            />
                            <circle class={`edot ${link.kind}${on ? '' : ' off'}`} cx={(from.x + to.x) / 2} cy={(from.y + to.y) / 2} r="3" />
                        </g>
                    );
                })}

                {/* Карточки: одна высота у всех, значки умений — сверху посередине (строка под них зарезервирована
                    всегда), заголовок и подпись — ниже; значки на карточке не меняют её размер. */}
                {cards.map((card) => (
                    <g key={card.id} class={`node ${cardLit(card)}`} transform={`translate(${card.x},${card.y})`}>
                        <rect class="node-box" x={-card.w / 2} y={-CARD_H / 2} width={card.w} height={CARD_H} rx="15" />
                        {card.caps && (
                            <g transform={`translate(${(-(card.caps.length - 1) * 18) / 2 - 6.5}, -22)`}>
                                {card.caps.map((cap) => (
                                    <g key={cap} class={`cap${CAP_ON[cap](solved) ? ' on' : ''}`} transform={`translate(${card.caps!.indexOf(cap) * 18}, 0)`}>
                                        <title>{CAP_TITLES[cap]}</title>
                                        <CapIcon cap={cap} />
                                    </g>
                                ))}
                            </g>
                        )}
                        {card.brands && (
                            <g transform={`translate(${(-(card.brands.length - 1) * 18) / 2 - 6.5}, -22)`}>
                                {card.brands.map((id) => (
                                    <g key={id} transform={`translate(${card.brands!.indexOf(id) * 18}, 0)`}>
                                        <BrandGlyph id={id} on={BRAND_OF_NAME[brainName ?? ''] === id} />
                                    </g>
                                ))}
                            </g>
                        )}
                        {card.alice && (
                            <g transform="translate(-6.5, -22)">
                                <AliceGlyph on={yandex && !solved.silent} />
                            </g>
                        )}
                        <text class="node-title" x={-card.w / 2 + 9} y={6}>
                            {card.title}
                        </text>
                        {card.sub && (
                            <text class="node-sub" x={-card.w / 2 + 9} y={20}>
                                {card.sub}
                            </text>
                        )}
                    </g>
                ))}

                <circle ref={packet} class="packet" r="5" cx={-20} cy={-20} style={{ opacity: run.length && !reduced ? 1 : 0 }} />
                <g class="badge" style={badge(gatewayCard, (gateway?.w ?? 100) / 2 - 10, -24)}>
                    <svg x={-12} y={-12} width={24} height={24} viewBox="0 0 28 28" aria-hidden="true">
                        <DuoWaveMark size={28} />
                    </svg>
                </g>
                <g class="badge" style={badge(gearCard, -(at.get(gearCard)?.w ?? 100) / 2 + 10, -24)}>
                    <circle class="badge-ring" r="12" />
                    <svg x={-8} y={-8} width={16} height={16} viewBox="0 0 16 16" style={{ color: 'var(--text)' }}>
                        <Gear />
                    </svg>
                </g>
            </svg>
            <div class="map-legend">
                <span>
                    <i class="dot own" aria-hidden="true" />
                    остаётся дома
                </span>
                <span>
                    <i class="dot out" aria-hidden="true" />
                    через интернет
                </span>
                <span>
                    <DuoWaveMark size={16} />
                    шлюз
                </span>
                <span>
                    <Gear size={14} />
                    настройки
                </span>
            </div>
            <p class="map-cap">{text}</p>
        </div>
    );
}

/** «Слышит · Думает · Говорит» — три плашки цвета того, кто делает работу. */
export function WhoThree({ ear, brain, voice }: { ear: Who; brain: Who; voice: Who }) {
    const tone = (who: Who) => (who.tone === 'cloud' ? 'tone-yandex' : `tone-${who.tone}`);
    return (
        <div class="who3">
            {(
                [
                    ['Слышит', ear],
                    ['Думает', brain],
                    ['Говорит', voice],
                ] as const
            ).map(([label, who]) => (
                <div key={label} class={tone(who)} title={who.sub}>
                    <span class="k">{label}</span>
                    <span class="v">{who.value}</span>
                </div>
            ))}
        </div>
    );
}
