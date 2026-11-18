// Первые шаги, шаг «Попробуйте сказать» — холст «Онбординг» 1:1 (Try / TryStation, на телефоне PhoneTry). Слева шесть
// фраз, справа сцена: как это делается — виджет «Курс доллара» (рисуется в рамке-комете, встаёт на стол, потом с ним
// говорят), игра «Неоновая змейка» (код в Мастерской → поле), поиск в интернете, память, связи вокруг Маши и сон.
// Это показ, а не настоящие вызовы: ходы идут по таймерам холста. Слово — «Алиса, …» у колонки, «ДиВи, …» у компьютера.
// Без движения (prefers-reduced-motion) показ сразу стоит на последнем ходе.
//
// Сцена холста — 744×620 (её место на макете 1280×800). Здесь она рисуется в этом же размере и сжимается под ширину
// рамки (--ot-k), так что на ноутбуке выглядит как на холсте. Узко (телефон) — фразы сверху, сцена меньше, а под ней то
// же словами, как на холсте «Телефон: попробуйте».

import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { tr } from '../../i18n';
import { useCallWord } from '../call-word';
import { useOnboarding, useStepNav } from './nav';

import '../../ui/onb-try.css';

const DEMOS = ['widget', 'game', 'search', 'memory', 'links', 'sleep'] as const;

/** Когда (мс от начала) показ переходит на следующий ход — как в холсте. */
const PLAN: number[][] = [
    [1300, 3100, 5000, 6400, 9000, 10600, 12600],
    [1300, 3900, 6300, 7600],
    [1300, 3900, 5600],
    [1400, 3200, 4600],
    [1200, 3400, 5000],
    [1500, 4400, 5600],
];

/** Ширина сцены на холсте (высота 620 — в onb-try.css). */
const STAGE_W = 744;
/** Уже этого — раскладка телефона: фразы над сценой. */
const NARROW = 760;

/** Сколько смотреть на конец показа, прежде чем сам начнётся следующий пример. */
const HOLD = 3200;

const reduced = (): boolean => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/** Значки строк — как на холсте. */
const ROW_ICONS: ComponentChildren[] = [
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="3" width="8" height="8" rx="2" />
        <rect x="13" y="3" width="8" height="8" rx="2" />
        <rect x="3" y="13" width="8" height="8" rx="2" />
        <path d="M17 13v8M13 17h8" />
    </svg>,
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M6 9h12a4 4 0 0 1 4 4v1a3 3 0 0 1-5.3 1.9L15 14H9l-1.7 1.9A3 3 0 0 1 2 14v-1a4 4 0 0 1 4-4z" />
        <path d="M7 11.5v3M5.5 13h3" />
    </svg>,
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="11" cy="11" r="7" />
        <path d="M20 20l-4-4" />
    </svg>,
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M6 3h12v18l-6-4-6 4z" />
    </svg>,
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="6" cy="12" r="2.5" />
        <circle cx="18" cy="6" r="2.5" />
        <circle cx="18" cy="18" r="2.5" />
        <path d="M8.3 11l7.4-3.8M8.3 13l7.4 3.8" />
    </svg>,
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />
    </svg>,
];

const IconBookmark = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M6 3h12v18l-6-4-6 4z" />
    </svg>
);

/** Задержка появления строки (код, список, сон). */
const delay = (s: number) => ({ animationDelay: `${s}s` });

export function TryStep() {
    const { wake } = useOnboarding();
    const station = wake === 'station';
    const t = tr.onbTry;
    // Слово, которое на деле зовёт (п. 54): стоит приложение-слово — его, иначе прежнее «Алиса» / «ДиВи».
    const word = useCallWord();
    const name = (station && word) || t.wake[wake];
    const ask = (text: string) => t.ask(name, text);

    // Показ: какая фраза и какой ход
    const [demo, setDemo] = useState(0);
    const [phase, setPhase] = useState(0);
    /** Каждый запуск показа — сцена заново, анимации с начала («Ещё раз»). */
    const [run, setRun] = useState(0);
    const timers = useRef<number[]>([]);
    const clear = () => {
        timers.current.forEach((timer) => window.clearTimeout(timer));
        timers.current = [];
    };
    const later = (ms: number, next: number) => timers.current.push(window.setTimeout(() => setPhase(next), ms));
    /** Конец показа: подержать и самому перейти к следующему примеру; на последнем — остановиться. */
    const then = (index: number, at: number) => {
        if (index < DEMOS.length - 1) timers.current.push(window.setTimeout(() => play(index + 1), at + HOLD));
    };
    const play = (index: number) => {
        clear();
        setDemo(index);
        setRun((r) => r + 1);
        if (reduced()) {
            setPhase(PLAN[index].length);
            then(index, 2000);
            return;
        }
        setPhase(0);
        PLAN[index].forEach((ms, i) => later(ms, i + 1));
        then(index, PLAN[index][PLAN[index].length - 1]);
    };
    // «На стол» на плашке виджета: виджет встал — дальше с ним говорят.
    const land = () => {
        clear();
        if (reduced()) {
            setPhase(7);
            return;
        }
        setPhase(5);
        later(1600, 6);
        later(3600, 7);
        then(0, 3600);
    };
    useEffect(() => {
        play(0);
        return clear;
    }, []);
    // «Дальше» листает примеры; после последнего — следующий шаг.
    useStepNav(
        {
            onNext: () => {
                if (demo >= DEMOS.length - 1) return;
                play(demo + 1);
                return false;
            },
        },
        [demo],
    );

    // Сцена держит размер холста и сжимается под ширину; узко — раскладка телефона.
    const root = useRef<HTMLDivElement>(null);
    const box = useRef<HTMLElement>(null);
    const [narrow, setNarrow] = useState(false);
    const [scale, setScale] = useState(1);
    useLayoutEffect(() => {
        const measure = () => {
            if (root.current) setNarrow(root.current.clientWidth < NARROW);
            if (box.current && box.current.clientWidth > 0) setScale(box.current.clientWidth / STAGE_W);
        };
        measure();
        if (typeof ResizeObserver === 'undefined') return;
        const observer = new ResizeObserver(measure);
        if (root.current) observer.observe(root.current);
        if (box.current) observer.observe(box.current);
        return () => observer.disconnect();
    }, []);

    // Состояние того, кто слушает, — как в холсте.
    const busy = (demo === 0 && phase >= 1 && phase <= 3) || (demo === 1 && phase >= 1 && phase <= 2) || (demo === 2 && phase === 1);
    const done =
        (demo === 0 && phase >= 4) ||
        (demo === 1 && phase >= 3) ||
        (demo === 2 && phase >= 2) ||
        (demo === 3 && phase >= 1) ||
        (demo === 4 && phase >= 2) ||
        (demo === 5 && phase >= 3);
    const asleep = demo === 5 && phase <= 1;
    let status = phase === 0 ? t.status.hearing : busy ? t.status.busy : done ? t.status.done : t.status.thinking;
    if (asleep) status = t.status.asleep;
    if (demo === 5 && phase === 2) status = t.status.awake;
    const diviCls = 'divi ot-divi ' + (asleep ? 'sleep' : phase === 0 || busy ? 'think' : done ? 'hop' : 'blink');

    const phrase = (i: number) => {
        const d = t.demos[DEMOS[i]];
        return 'phraseStation' in d && station ? d.phraseStation : d.phrase;
    };

    const is = (index: number, from: number, to = Infinity) => demo === index && phase >= from && phase <= to;
    const w = {
        comet: is(0, 1, 2),
        sk: is(0, 2, 2),
        card: is(0, 3),
        making: is(0, 1, 3),
        ask: is(0, 4, 4),
        landed: is(0, 5, 5),
        talk: is(0, 6),
        watch: is(0, 7),
    };
    const g = { code: is(1, 1, 2), todo: is(1, 2, 2), game: is(1, 3), making: is(1, 1, 2), done: is(1, 4) };
    const s = { search: is(2, 1, 1), answer: is(2, 2), suggest: is(2, 3) };
    const m = { saved: is(3, 1), ask: is(3, 2), reply: is(3, 3) };
    const l = { graph: is(4, 1), reply: is(4, 2), more: is(4, 3) };
    const n = { night: demo === 5, rows: is(5, 1), morning: is(5, 3) };
    const showWho = !(demo === 0 && phase >= 7);
    const sayShow = demo !== 5 || phase >= 2;

    const keyCard = (
        <div class="card card-tight ot-key">
            {station ? (
                <>
                    <span class="chip chip-lime">{t.stationChip(name)}</span>
                    <span class="small">{t.stationBody}</span>
                </>
            ) : (
                <>
                    <span class="ot-kchips">
                        <span class="chip ot-kchip">{t.keyAlt}</span>
                        <span class="chip ot-kchip">{t.keySpace}</span>
                    </span>
                    <span class="small">{t.keyBody}</span>
                </>
            )}
        </div>
    );
    const phone = t.phone[DEMOS[demo]];

    return (
        <div ref={root} class={`ot${narrow ? ' narrow' : ''}`}>
            <section class="ot-side">
                <h1 class="h2">{t.title}</h1>
                <p class="small ot-how">{station ? t.howStation : t.howComputer}</p>
                <div class="ot-rows">
                    {DEMOS.map((id, i) => (
                        <button
                            key={id}
                            type="button"
                            class={`row-btn${demo === i ? ' on' : ''}`}
                            aria-pressed={demo === i}
                            onClick={() => play(i)}
                        >
                            <span class="n">{ROW_ICONS[i]}</span>
                            <span class="ot-rowt">
                                <span>{ask(phrase(i))}</span>
                                <span class="small ot-kind">{t.demos[id].kind}</span>
                            </span>
                        </button>
                    ))}
                </div>
                {!narrow && keyCard}
            </section>

            <section ref={box} class="stage ot-box" aria-label={t.stageLabel}>
                <div key={run} class="ot-c" style={`--ot-k: ${scale}`}>
                    {sayShow && (
<div class="say ot-say1">{ask(phrase(demo))}</div>
                    )}

                    {/* 1. Виджет */}
                    {w.comet && (
                        <div class="comet ot-comet">
                            <div class="comet-in">
                                <span class="small ot-agent">{t.widget.drawing}</span>
                                {w.sk && (
                                    <>
                                        <span class="sk ot-sk1" />
                                        <span class="sk ot-sk2" />
                                        <span class="sk ot-sk3" />
                                    </>
                                )}
                            </div>
                        </div>
                    )}
                    {w.card && (
                        <div class="widget land ot-wcard">
                            <div class="widget-h">
                                <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                    <path d="M4 17l5-5 4 3 7-8" />
                                    <path d="M15 7h5v5" />
                                </svg>
                                {t.widget.name}
                                <span class="widget-src">{t.widget.kind}</span>
                            </div>
                            <div class="ot-wrow">
                                <div>
                                    <div class="num">{t.widget.price}</div>
                                    <div class="small ot-up">{t.widget.change}</div>
                                    {w.watch && (
                                        <div class="land ot-eur">
                                            {t.widget.euro} <b>{t.widget.euroPrice}</b>{' '}
                                            <span class="small ot-dn">{t.widget.euroChange}</span>
                                        </div>
                                    )}
                                </div>
                                <svg width="120" height="48" viewBox="0 0 120 48" aria-hidden="true">
                                    <polyline class="ot-spark" points="0,38 15,34 30,36 45,26 60,29 75,20 90,23 105,12 120,8" />
                                </svg>
                            </div>
                        </div>
                    )}
                    {w.making && (
                        <div class="plaque ot-br">
                            <span class="plaque-v" />
                            {t.widget.making}
                        </div>
                    )}
                    {w.ask && (
                        <div class="plaque ot-br">
                            {t.widget.ready}
                            <button type="button" class="dw-btn dw-btn-accent" onClick={land}>
                                {t.widget.toDesk}
                            </button>
                            <button type="button" class="dw-btn dw-btn-light" tabIndex={-1}>
                                {t.widget.tryOn}
                            </button>
                        </div>
                    )}
                    {w.landed && (
                        <div class="hint ot-rise ot-landed">
                            <i />
                            <span>{t.widget.landed}</span>
                        </div>
                    )}
                    {w.talk && <div class="say ot-sayw">{ask(t.widget.talk)}</div>}
                    {w.watch && (
                        <>
                            <div class="plaque ot-still ot-watch">
                                <span class="plaque-v" />
                                {t.widget.watch}
                            </div>
                            <div class="sheet ot-still ot-help">
                                <div class="h3 ot-h3s">{t.widget.helpTitle}</div>
                                <div class="small ot-help-b">
                                    {ask(t.widget.help1)}
                                    {t.widget.help1Tail}
                                    <br />
                                    {ask(t.widget.help2)}
                                    {t.widget.help2Tail}
                                    <br />
                                    {ask(t.widget.help3)}
                                    {t.widget.help3Tail}
                                </div>
                                <div class="small ot-fine ot-help-n">{t.widget.helpNote}</div>
                            </div>
                        </>
                    )}

                    {/* 2. Игра */}
                    {g.code && (
                        <div class="win ot-codewin">
                            <div class="win-h">
                                <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                    <path d="M8 7l-5 5 5 5M16 7l5 5-5 5" />
                                </svg>
                                {t.game.workshop}
                            </div>
                            <div class="code" aria-hidden="true">
                                <div style={delay(0.1)}>
                                    <span class="c">{t.game.codeComment}</span>
                                </div>
                                <div style={delay(0.4)}>
                                    <span class="k">const</span>
                                    {' field = '}
                                    <span class="f">grid</span>
                                    {'(20, 20);'}
                                </div>
                                <div style={delay(0.7)}>
                                    <span class="k">let</span>
                                    {' snake = [{ x: 4, y: 10 }], dir = '}
                                    <span class="s">{"'right'"}</span>;
                                </div>
                                <div style={delay(1)}>
                                    <span class="f">onKey</span>(<span class="s">{"'arrows'"}</span>
                                    {', (key) => dir = key);'}
                                </div>
                                <div style={delay(1.3)}>
                                    <span class="f">every</span>
                                    {'(120, () => {'}
                                </div>
                                <div style={delay(1.6)}>
                                    {'  snake = '}
                                    <span class="f">step</span>
                                    {'(snake, dir);'}
                                </div>
                                <div style={delay(1.9)}>
                                    {'  '}
                                    <span class="k">if</span> (<span class="f">ate</span>
                                    {'(snake)) score += 1;'}
                                </div>
                                <div style={delay(2.2)}>
                                    {'  '}
                                    <span class="f">glow</span>
                                    {'(snake, '}
                                    <span class="s">{"'#b6f36a'"}</span>
                                    {');'}
                                </div>
                                <div style={delay(2.4)}>{'});'}</div>
                            </div>
                            {g.todo && (
                                <div class="todo">
                                    <span style={delay(0.1)}>
                                        <span class="c-own">✓</span> {t.game.todo1}
                                    </span>
                                    <span style={delay(0.5)}>
                                        <span class="c-own">✓</span> {t.game.todo2}
                                    </span>
                                    <span style={delay(0.9)}>
                                        <span class="c-own">✓</span> {t.game.todo3}
                                    </span>
                                    <span style={delay(1.3)}>
                                        <span class="dot dot-busy" /> {t.game.todoCheck}
                                    </span>
                                </div>
                            )}
                        </div>
                    )}
                    {g.game && (
                        <div class="win land ot-game">
                            <div class="win-h">
                                <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                    <path d="M6 9h12a4 4 0 0 1 4 4v1a3 3 0 0 1-5.3 1.9L15 14H9l-1.7 1.9A3 3 0 0 1 2 14v-1a4 4 0 0 1 4-4z" />
                                </svg>
                                {t.game.name}
                                <span class="ot-score">{t.game.score}</span>
                            </div>
                            <div class="field-snake" aria-hidden="true">
                                <div class="snake">
                                    <i />
                                    <i />
                                    <i />
                                    <i />
                                </div>
                                <span class="food" />
                            </div>
                            <div class="small ot-keys">{t.game.keys}</div>
                        </div>
                    )}
                    {g.making && (
                        <div class="plaque ot-br">
                            <span class="plaque-v" />
                            {t.game.making}
                        </div>
                    )}
                    {g.done && (
                        <div class="plaque ot-br">
                            {t.game.done}
                            <button type="button" class="dw-btn dw-btn-accent" tabIndex={-1}>
                                {t.game.play}
                            </button>
                        </div>
                    )}

                    {/* 3. Поиск */}
                    {s.search && (
                        <div class="sheet ot-rise ot-search">
                            <span class="ot-line">
                                <span class="mark mark-busy ot-busy" />
                                <span class="h3">{t.search.searching}</span>
                            </span>
                            <span class="ot-srcs">
                                <span class="src" style={delay(0.2)}>
                                    {t.search.source1}
                                </span>
                                <span class="src" style={delay(0.7)}>
                                    {t.search.source2}
                                </span>
                                <span class="src" style={delay(1.2)}>
                                    {t.search.source3}
                                </span>
                            </span>
                            <span class="small ot-rule">{t.search.note}</span>
                        </div>
                    )}
                    {s.answer && (
                        <>
                            <div class="widget land ot-films">
                                <div class="widget-h">
                                    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                        <rect x="3" y="5" width="18" height="14" rx="2" />
                                        <path d="M3 9h18M8 5v4M16 5v4" />
                                    </svg>
                                    {t.search.title}
                                    <span class="widget-src">{t.search.sources}</span>
                                </div>
                                <div class="ot-films-l">
                                    {(
                                        [
                                            ['face-cool', t.search.film1, t.search.film1About],
                                            ['face-violet', t.search.film2, t.search.film2About],
                                            ['face-warm', t.search.film3, t.search.film3About],
                                        ] as const
                                    ).map(([face, title, about]) => (
                                        <div key={face} class="film">
                                            <span class={`poster ${face}`} />
                                            <span>
                                                <span class="q">{title}</span>
                                                <br />
                                                <span class="small">{about}</span>
                                            </span>
                                        </div>
                                    ))}
                                </div>
                                <div class="small ot-fine ot-rule ot-card-note">{t.search.note}</div>
                            </div>
                            <div class="reply ot-reply-s">{t.search.reply}</div>
                        </>
                    )}
                    {s.suggest && (
                        <div class="plaque ot-br">
                            {t.search.suggest}
                            <button type="button" class="dw-btn dw-btn-accent" tabIndex={-1}>
                                {t.search.make}
                            </button>
                        </div>
                    )}

                    {/* 4. Память */}
                    {m.saved && (
                        <div class="remember ot-saved">
                            <IconBookmark />
                            {t.memory.saved}
                            <button type="button" class="btn-ghost ot-undo" tabIndex={-1}>
                                {t.memory.undo}
                            </button>
                        </div>
                    )}
                    {m.ask && <div class="say ot-say2">{ask(t.memory.ask)}</div>}
                    {m.reply && (
                        <>
                            <div class="reply ot-reply-m">{t.memory.reply}</div>
                            <div class="sheet ot-rise ot-home">
                                <span class="mark mark-ok ot-home-mark">
                                    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                        <path d="M4 11l8-7 8 7v9H4z" />
                                        <path d="M10 20v-5h4v5" />
                                    </svg>
                                </span>
                                <span class="ot-col4">
                                    <span class="h3 ot-h3s">{t.memory.homeTitle}</span>
                                    <span class="small">{t.memory.homeBody}</span>
                                    <span class="small ot-fine">{t.memory.homeNote}</span>
                                </span>
                            </div>
                        </>
                    )}

                    {/* 5. Связи */}
                    {l.graph && (
                        <>
                            <svg class="ot-graph" width="740" height="616" viewBox="0 0 740 616" aria-hidden="true">
                                <path class="gline-new" d="M340 236 L190 140" />
                                <path class="gline" d="M340 236 L520 140" />
                                <path class="gline" d="M340 236 L570 250" />
                                <path class="gline" d="M340 236 L190 340" />
                                <path class="gline" d="M340 236 L500 350" />
                            </svg>
                            <span class="gnode main ot-g0">{t.links.main}</span>
                            <span class="gnode ot-g1">
                                <IconBookmark />
                                {t.links.allergy}
                            </span>
                            <span class="gnode ot-g2">{t.links.brother}</span>
                            <span class="gnode ot-g3">{t.links.cat}</span>
                            <span class="gnode ot-g4">{t.links.birthday}</span>
                            <span class="gnode infer ot-g5">
                                {t.links.infer}
                                <span class="ot-why">{t.links.why}</span>
                            </span>
                        </>
                    )}
                    {l.reply && <div class="reply ot-reply-l">{t.links.reply}</div>}
                    {l.more && <span class="note-grey ot-more">{t.links.more}</span>}

                    {/* 6. Сон */}
                    {n.night && (
                        <div class="night ot-night">
                            <div class="ot-line">
                                <svg class="ico" viewBox="0 0 24 24" aria-hidden="true">
                                    <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />
                                </svg>
                                <span class="h3">{t.sleep.time}</span>
                            </div>
                            <p class="small ot-night-p">{t.sleep.dream}</p>
                            {n.rows &&
                                [t.sleep.row1, t.sleep.row2, t.sleep.row3, t.sleep.row4].map((row, i) => (
                                    <div key={i} class="night-row" style={delay(0.1 + i * 0.6)}>
                                        <b>{i + 1}</b>
                                        {row}
                                    </div>
                                ))}
                        </div>
                    )}
                    {n.morning && (
                        <>
                            <div class="widget land ot-morning">
                                <div class="widget-h">
                                    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                        <circle cx="12" cy="12" r="4" />
                                        <path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5L19 19M5 19l1.5-1.5M17.5 6.5L19 5" />
                                    </svg>
                                    {t.sleep.title}
                                    <span class="widget-src">{t.sleep.where}</span>
                                </div>
                                <div class="small ot-summary">{t.sleep.summary}</div>
                                <div class="ot-choose">
                                    <span class="gnode infer ot-infer">{t.sleep.infer}</span>
                                    <button type="button" class="dw-btn dw-btn-dark dw-btn-sm" tabIndex={-1}>
                                        {t.sleep.accept}
                                    </button>
                                    <button type="button" class="dw-btn dw-btn-light dw-btn-sm" tabIndex={-1}>
                                        {t.sleep.reject}
                                    </button>
                                </div>
                            </div>
                            <span class="note-grey ot-sleep-note">{t.sleep.note}</span>
                        </>
                    )}

                    {/* Кто слушает */}
                    {showWho && (
                        <div class="ot-who">
                            {station ? (
                                <span class="speaker ot-speaker">
                                    <i />
                                </span>
                            ) : (
                                <span class={diviCls}>
                                    <svg viewBox="0 0 256 256" aria-hidden="true">
                                        <path
                                            class="divi-body"
                                            d="M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z"
                                        />
                                        <path class="divi-eye divi-eyes" d="M98 110V138M158 110V138" />
                                    </svg>
                                </span>
                            )}
                            <span class="small" aria-live="polite">
                                {station ? t.status.station(status) : status}
                            </span>
                        </div>
                    )}

                    <button type="button" class="btn-ghost ot-again" onClick={() => play(demo)}>
                        {t.again}
                    </button>
                </div>
            </section>

            {narrow && (
                <div key={demo} class="sheet land ot-phone">
                    <span class="h3">{phone.title}</span>
                    <span class="small">{phone.body}</span>
                    {phone.grey && <span class="small ot-grey">{phone.grey}</span>}
                </div>
            )}
            {narrow && keyCard}
        </div>
    );
}
