// «Первые шаги» (docs/plans/onboarding.md): один экран во весь пульт после пароля — 1:1 как холст «Онбординг» (его
// стиль — ui/onboarding-canvas.css). Рамка здесь: шапка с ДиВи и языком, середина с шагом, низ с полосой шагов и кнопками
// «Назад» · «Дальше». Второй шаг — «Где будет жить DuoWave»: только компьютер, колонка + компьютер или только колонка;
// от него зависят остальные (screens/onb/*). Шаг говорит с рамкой через screens/onb/nav.tsx. Где остановились, помнит
// память пульта (shell/memory.ts): закрыли на середине — стол, вернуться можно из «Настроек» → «Оформление».

import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, FoundStation, PairCodeAsked, PairCodeLook, StationGatewayView, api, can, problemOf, stationCall } from '../api';
import { drawingOf } from '../stations';
import { LANGUAGES, setLanguage, tr, useLanguage } from '../i18n';
import { usePanel } from '../shell/context';
import { flushPanelMemory, recalled, remember } from '../shell/memory';
import { OnbRecord, RECORD, closeRecord, openRecord, patchRecord, readRecord } from './onb/record';
import { Boundary } from '../shell/slot';
import { noteVoiceInput } from '../voice-input';
import { AnswerStep } from './onb/answer';
import { BrainStep } from './onb/brain';
import { CharacterStep } from './onb/character';
import { Choice, OnboardingContext, StepNav, StepNavContext } from './onb/nav';
import { SpeakerStep, avoidStations, resumeLink } from './onb/speaker';
import { StoreStep } from './onb/store';
import { TryStep } from './onb/try';
import { TurboStep } from './onb/turbo';
import { StatusStep } from './onb/status';
import { ONBOARDING, TOP_VIA, VIA } from '../via';
import { InstallPill } from '../magicplace/wizard';
import '../ui/onboarding-canvas.css';

type StepId = 'where' | 'brain' | 'speaker' | 'answer' | 'character' | 'try' | 'store' | 'status';

/**
 * Запись первых шагов (onb/record.ts): выбор, шаг, пройденные шаги и хвосты. Нет записи — шаги пройдены или закрыты.
 * Запись появляется с первого экрана: смена языка пересоздаёт пульт (main.tsx), перезапуск приложения или шлюза — и
 * шаги открываются снова по ней, на том же шаге. `picked: false` — место выбрал не человек, а пульт (запись постарше
 * без поля — выбрал человек): такой выбор не держим, по умолчанию — «колонка + Турбо» (замечание 1 теста 19.11).
 */
interface Saved extends OnbRecord {
    choice: Choice;
    step: StepId;
}
const KEY = RECORD;

const WIKI_ROOT = 'https://hackalice.me/start/';

/**
 * «Пройти заново» поверх настроенного дома (замечание 10 теста 19.11): место — каким оно есть (колонка у этого
 * компьютера — «колонка + Турбо», иначе «этот компьютер»), и отмечено, что уже сделано: место, колонка подключена,
 * мозг выбран. Данные дома не трогаются; каждый шаг показывает своё как есть, поменять можно, а можно «Дальше».
 */
function againStart(panel: ReturnType<typeof usePanel>): { choice: Choice; done: StepId[]; picked: boolean } {
    const linked = panel.role.role === 'helper' || (panel.health?.placement?.stations ?? []).length > 0;
    const thinks = !!panel.health?.brain && panel.health.brain.provider !== 'none' && panel.health.brain.provider !== 'echo';
    const done: StepId[] = ['where', ...(linked ? (['speaker', 'answer'] as StepId[]) : []), ...(thinks ? (['brain'] as StepId[]) : [])];
    return { choice: linked ? 'stationPc' : 'computer', done, picked: true };
}

/** Шаги по выбору. Шлюз на самой колонке (пульт открыт на ней): где жить — уже решено. */
function stepsOf(choice: Choice, onStation: boolean): StepId[] {
    // «Ваш дом» (этап 4 плана home-setup-flow) — последним на каждом пути.
    if (onStation) return ['character', 'try', 'store', 'status'];
    if (choice === 'computer') return ['where', 'brain', 'character', 'try', 'store', 'status'];
    return ['where', 'speaker', 'answer', 'character', 'try', 'store', 'status'];
}

/**
 * Открыть первые шаги из «Настроек» (или откуда угодно): пульт слушает это событие (app.tsx). В окне колонки у
 * помощника их нет — шаги ведёт пульт компьютера: окно уходит туда с просьбой `?onboarding` (via.ts).
 */
export const OPEN_ONBOARDING = 'duowave:onboarding';
export function openOnboarding(): void {
    // Окно колонки: шаги ведёт пульт компьютера — адрес `?onboarding` там и значит «пройти заново» (см. Onboarding).
    if (VIA) {
        const top = TOP_VIA ? window : (window.top ?? window);
        top.location.assign(`${location.pathname}?onboarding`);
        return;
    }
    // «Пройти заново» (замечание 10 теста 19.11): поверх того, что уже есть, — дом не трогается, сделанное отмечено.
    // Шаги не закончены (запись есть) — открываются как были.
    if (!readRecord()) remember(KEY, { again: true } as Partial<Saved>);
    window.dispatchEvent(new Event(OPEN_ONBOARDING));
}

/** Первые шаги не закончены: пульт после перезагрузки открывает их снова, на том же шаге. */
export function onboardingPending(): boolean {
    return !!recalled<Saved>(KEY);
}

// ─── Значки холста ─────────────────────────────────────────────

const ICONS = {
    laptop: (
        <>
            <rect x="3" y="5" width="18" height="11" rx="2" />
            <path d="M2 19h20" />
        </>
    ),
    speaker: (
        <>
            <rect x="6" y="3" width="12" height="18" rx="6" />
            <path d="M9.5 15h5" />
        </>
    ),
    speakerOnly: (
        <>
            <rect x="6" y="3" width="12" height="18" rx="6" />
            <path d="M9.5 15h5" />
            <circle cx="12" cy="9" r="1.2" />
        </>
    ),
    check: <path d="M5 12.5l4.5 4.5L19 7.5" />,
    warn: <path d="M12 7v6M12 16.5v.5" />,
    wave: <path d="M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" />,
    mic: (
        <>
            <rect x="9" y="3" width="6" height="11" rx="3" />
            <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
        </>
    ),
    spark: <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" />,
    close: <path d="M6 6l12 12M18 6 6 18" />,
};

function Ico({ name, size = '' }: { name: keyof typeof ICONS; size?: 'sm' | 'lg' | '' }) {
    return (
        <svg class={`ico${size ? ` ico-${size}` : ''}`} viewBox="0 0 24 24" aria-hidden="true">
            {ICONS[name]}
        </svg>
    );
}

/** ДиВи холста: лайм и глаза-капсулы, моргает. */
export function Divi({ size = 26, mood = 'blink' }: { size?: number; mood?: 'blink' | 'think' | 'hop' | 'sleep' }) {
    return (
        <span class={`divi ${mood}`} style={{ width: `${size}px`, height: `${size}px` }}>
            <svg viewBox="0 0 256 256" aria-hidden="true">
                <path class="divi-body" d="M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z" />
                <path class="divi-eye divi-eyes" d="M98 110V138M158 110V138" />
            </svg>
        </span>
    );
}

// ─── Рамка ─────────────────────────────────────────────────────

export function Onboarding({ onClose }: { onClose: () => void }) {
    const panel = usePanel();
    const current = useLanguage();
    const onStation = panel.health?.placement?.role === 'station';
    const saved = recalled<Saved>(KEY);
    // «Пройти заново»: дом уже есть — место такое, какое оно на деле, и отмечено, что уже сделано (`againStart`).
    // Адрес `?onboarding` (просьба окна колонки «Пройти первые шаги») без начатых шагов — то же «заново».
    const again = (saved?.again && !saved.choice) || (ONBOARDING && !saved) ? againStart(panel) : null;
    // По умолчанию — «Центр — колонка, компьютер — Турбо», найдена колонка или нет: дом задуман вокруг колонки, а без
    // найденной шаг «Колонка» даёт ввести её адрес. Держим только выбор человека.
    const [choice, setChoice] = useState<Choice>(
        again ? again.choice : saved?.choice && saved.picked !== false ? saved.choice : 'stationPc',
    );
    const [done, setDone] = useState<string[]>(again?.done ?? saved?.done ?? []);
    const steps = stepsOf(choice, onStation);
    const [step, setStep] = useState<StepId>(saved && steps.includes(saved.step) ? saved.step : steps[0]);
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const [nav, setNav] = useState<StepNav | null>(null);
    // Компьютер вошёл в дом по коду: шаг «Что даёт этот компьютер» (этап 3 плана home-setup-flow), потом «Ваш дом» и стол
    // дома. Место держит запись шагов (`joined`): повторный вход после копии дома продолжает отсюда.
    const [turbo, setTurboNow] = useState<{ key: string; name: string; status?: boolean } | null>(again ? null : (saved?.joined ?? null));
    const setTurbo = (next: { key: string; name: string; status?: boolean }) => {
        setTurboNow(next);
        patchRecord({ joined: next });
    };
    const main = useRef<HTMLElement>(null);

    // Подключение и перенос колонки ведёт пульт (screens/onb/speaker.tsx), а не шлюз: уход со шага их не останавливает,
    // а перезагрузка пульта (шлюз перезапустился) теряла — продолжаем по метке, на каком бы шаге ни были (п. 62).
    useEffect(() => void resumeLink().catch(() => undefined), []);
    // Шаги открыты — пульт помнит это сразу, ещё до «Дальше» (см. Saved). «Пройти заново» — место выбрано по дому.
    useEffect(() => {
        if (again) remember(KEY, { ...saved, ...again, step, again: true } satisfies Saved);
        else openRecord({ choice, step, picked: false });
    }, []);

    // Колонки дома: поиск начинается сразу; выбор он не меняет (см. выше).
    const scan = useScan(!onStation);

    // Компьютер уже в доме, а шаги ещё на «Где живёт» (дом настроили в пульте колонки, потом вошли сюда по паролю или
    // коду — замечание 5 после 0.18.1): шаги колонки — её, пройдены там. Сразу последний шаг — «Ваш дом: что работает», с
    // кнопками у того, что не так; центр — компьютер — сразу стол. Свой путь «колонка + Турбо» (компьютер сам переносит
    // шлюз на колонку) сюда не попадает: он становится Турбо на шаге «Колонка», уже после «Где живёт». «Пройти заново» —
    // тоже нет: там человек сам идёт по шагам.
    const helperKey = panel.role.role === 'helper' ? (panel.role.station?.key ?? null) : undefined;
    useEffect(() => {
        if (onStation || again || turbo || helperKey === undefined || step !== 'where' || done.includes('speaker')) return;
        if (helperKey === null) {
            finish();
            panel.go('home');
            return;
        }
        setTurbo({ key: helperKey, name: panel.role.station?.name ?? '', status: true });
    }, [helperKey]);

    const index = Math.max(0, steps.indexOf(step));
    const go = (next: StepId) => {
        // Пройденный «Дальше» шаг — в запись: после перезапуска полоса шагов показывает его ✓.
        const passed = steps.indexOf(next) > index && !done.includes(step) ? [...done, step] : done;
        setStep(next);
        setDone(passed);
        setProblem(null);
        setNav(null);
        patchRecord({ choice, step: next, picked: undefined, done: passed });
        main.current?.scrollTo({ top: 0 });
    };
    const finish = () => {
        closeRecord();
        // Сразу на шлюз, не через паузу: перезагрузка сразу после × иначе вернула бы шаги.
        void flushPanelMemory();
        // Колонка могла получить запись шагов от пульта компьютера (до отдельных копий памяти, shell/memory.ts): шаги
        // пройдены — и там тоже, иначе её пульт (телефон) открыл бы их снова.
        const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
        if (station) void stationCall(station, 'PATCH', '/api/v1/panel/state', { [KEY]: null }).catch(() => undefined);
        onClose();
    };
    const forward = () => (index === steps.length - 1 ? finish() : go(steps[index + 1]));

    /** «Дальше»: сначала своё у шага; выбор места записывается сразу («только компьютер» — режим «Микрофон»). */
    const next = async () => {
        setBusy(true);
        setProblem(null);
        try {
            if (nav?.onNext) {
                const stay = (await nav.onNext()) === false;
                if (stay) return;
            }
            if (step === 'where' && can('station.mode')) {
                const chosen = await api.setTalkInput(choice === 'computer' ? 'computer' : 'station');
                noteVoiceInput(chosen.input);
            }
            forward();
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(false);
        }
    };
    const back = () => {
        if (nav?.onBack && nav.onBack() === false) return;
        if (index > 0) go(steps[index - 1]);
    };

    const helper = scan.withGateway[0];
    const helperShown = step === 'where' && !!helper && !scan.helperDeclined;
    const wake = choice === 'computer' && !onStation ? 'computer' : 'station';
    // Полоса шагов холста: «Пароль» уже пройден, дальше шаги этого пути.
    const labels = [tr.onboarding.steps.password, ...steps.map((id) => tr.onboarding.steps[id])];
    const where = onStation ? tr.onboarding.whereStation : choice === 'computer' && step !== 'where' ? tr.onboarding.whereComputer : '';
    return (
        <OnboardingContext.Provider
            value={{
                choice,
                wake,
                next: forward,
                leave: (section) => (finish(), section && panel.go(section)),
                goTo: (to) => steps.includes(to as StepId) && go(to as StepId),
            }}
        >
            <StepNavContext.Provider value={setNav}>
                <div class="onb" role="dialog" aria-modal="true" aria-label={tr.onboarding.title}>
                    <header class="onb-top">
                        <span class="ob-brand">
                            <Divi />
                            DuoWave
                        </span>
                        {where && <span class="ob-where">· {where}</span>}
                        {/* Свёрнутая установка (мастер п. 40): дока в первых шагах нет — её ход здесь, по нажатию мастер (п. 55). */}
                        <InstallPill place="onb" />
                        <div class="ob-lang" role="group" aria-label={tr.settings.languageLabel}>
                            {LANGUAGES.map((language) => (
                                <button key={language.code} type="button" lang={language.code} aria-pressed={current === language.code} onClick={() => void setLanguage(language.code)}>
                                    {language.name}
                                </button>
                            ))}
                        </div>
                        <button type="button" class="round-btn onb-x" aria-label={tr.onboarding.close} title={tr.onboarding.close} onClick={finish}>
                            <Ico name="close" size="sm" />
                        </button>
                    </header>

                    <main class="onb-main" ref={main}>
                        {(saved?.again || again) && index === 0 && (
                            <p class="small onb-again" role="status">
                                {tr.onboarding.againLead}
                            </p>
                        )}
                        {/* Встроенная часть упала (нет поля в ответе шлюза) — шаг говорит об этом, «Дальше» работает. */}
                        <Boundary key={step} name={`onboarding.${step}`} fallback={<p class="card small">{tr.onboarding.stepFailed}</p>}>
                            {turbo?.status ? (
                                <StatusStep
                                    centre={turbo.key}
                                    onDone={() => {
                                        finish();
                                        panel.go('home');
                                    }}
                                />
                            ) : turbo ? (
                                <TurboStep station={turbo.key} name={turbo.name} onDone={() => setTurbo({ ...turbo, status: true })} />
                            ) : helperShown ? (
                                <HelperStep
                                    found={helper}
                                    onNo={() => {
                                        // «Это новый дом» (этап 0 плана home-setup-flow): дом — на этом компьютере, колонку
                                        // в другом доме не трогаем: ни подключения мастера, ни переноса на неё.
                                        avoidStations(scan.withGateway.map((station) => station.address));
                                        scan.declineHelper();
                                        if (!scan.found.some((station) => !scan.withGateway.some((busy) => busy.address === station.address))) {
                                            setChoice('computer');
                                            patchRecord({ choice: 'computer', step, picked: undefined });
                                        }
                                    }}
                                    onLinked={(joined) => {
                                        // Центр — компьютер: он ведёт этот компьютер-Турбо сам, шагов колонки нет.
                                        if (joined.computer) {
                                            finish();
                                            panel.go('home');
                                        } else setTurbo(joined);
                                    }}
                                />
                            ) : step === 'where' ? (
                                <WhereStep
                                    choice={choice}
                                    scan={scan}
                                    onPick={(value) => {
                                        setChoice(value);
                                        patchRecord({ choice: value, step, picked: undefined });
                                    }}
                                />
                            ) : step === 'brain' ? (
                                <BrainStep />
                            ) : step === 'speaker' ? (
                                <SpeakerStep />
                            ) : step === 'answer' ? (
                                <AnswerStep />
                            ) : step === 'character' ? (
                                <CharacterStep />
                            ) : step === 'try' ? (
                                <TryStep />
                            ) : step === 'status' ? (
                                <StatusStep />
                            ) : (
                                <StoreStep />
                            )}
                        </Boundary>
                    </main>

                    {!helperShown && !turbo && (
                        <footer class={`onb-foot${nav?.secondary ? ' crowded' : ''}`}>
                            <div class="steps" aria-label={tr.onboarding.title}>
                                {labels.map((label, at) => {
                                    const n = at - 1;
                                    // Пройденный — до текущего или отмеченный в записи (после перезапуска, «Пройти заново»).
                                    const passed = at === 0 || n < index || (n >= 0 && done.includes(steps[n]));
                                    const now = n === index;
                                    return (
                                        <span key={label} class="onb-step-wrap">
                                            <span
                                                class={`step${passed && !now ? ' done' : now ? ' now' : ''}`}
                                                aria-current={now ? 'step' : undefined}
                                            >
                                                <i>{passed && !now ? '✓' : at + 1}</i>
                                                {label}
                                            </span>
                                            {at < labels.length - 1 && <span class="step-gap" />}
                                        </span>
                                    );
                                })}
                            </div>
                            <div class="onb-dots" aria-hidden="true">
                                {steps.map((id, at) => (
                                    <i key={id} class={at < index ? 'done' : at === index ? 'now' : ''} />
                                ))}
                            </div>
                            <nav class="ob-nav">
                                {problem && (
                                    <span class="small onb-problem" role="alert">
                                        {problem}
                                    </span>
                                )}
                                {(index > 0 || nav?.onBack) && !nav?.hideBack && (
                                    <button type="button" class="btn-ghost" onClick={back}>
                                        {tr.onboarding.back}
                                    </button>
                                )}
                                {nav?.secondary && (
                                    <button type="button" class="dw-btn dw-btn-lg dw-btn-light" onClick={nav.secondary.onClick}>
                                        {nav.secondary.label}
                                    </button>
                                )}
                                <button
                                    type="button"
                                    class="dw-btn dw-btn-lg dw-btn-accent onb-next"
                                    disabled={busy || !!nav?.disabled || !!nav?.busy}
                                    aria-busy={nav?.busy || undefined}
                                    onClick={() => void next()}
                                >
                                    {nav?.busy && <span class="mark mark-busy onb-next-spin" aria-hidden="true" />}
                                    {nav?.label ?? (index === steps.length - 1 ? tr.onboarding.toDesk : tr.onboarding.next)}
                                </button>
                            </nav>
                        </footer>
                    )}
                </div>
            </StepNavContext.Provider>
        </OnboardingContext.Provider>
    );
}

// ─── Поиск колонок ─────────────────────────────────────────────

interface Found extends FoundStation {
    /** Порт и версия шлюза DuoWave на колонке, если он там есть. */
    gatewayPort?: number;
    version?: string | null;
    /** Центр дома — компьютер или сервер (его объявление `_duowave._tcp`), а не колонка. */
    computer?: boolean;
}

interface Scan {
    done: boolean;
    /** Колонки в сети. */
    found: FoundStation[];
    /** Колонки, на которых уже работает шлюз DuoWave. */
    withGateway: Found[];
    helperDeclined: boolean;
    declineHelper: () => void;
}

function useScan(enabled: boolean): Scan {
    const panel = usePanel();
    const [found, setFound] = useState<FoundStation[] | null>(null);
    const [withGateway, setWithGateway] = useState<Found[]>([]);
    const [declined, setDeclined] = useState(false);
    // Компьютер уже связан с колонкой — помогать нечему предлагать.
    const linked = !!panel.health?.placement?.station || panel.role.role === 'helper';
    useEffect(() => {
        if (!enabled || !can('stations.manage')) {
            setFound([]);
            return;
        }
        let alive = true;
        void (async () => {
            try {
                const { stations, homes = [] } = await api.discoverStations();
                if (!alive) return;
                setFound(stations);
                if (linked) return;
                const probed = await Promise.all(
                    stations.slice(0, 6).map(async (station): Promise<Found | null> => {
                        const probe = await api.probeStation(station.address).catch(() => null);
                        if (!probe?.gateway) return null;
                        // Ухо дома (этап 6): дом — у его центра, его и предлагаем, а не ухо.
                        if (probe.earOf) {
                            const centre = await api.probeStation(probe.earOf.address, probe.earOf.port).catch(() => null);
                            return centre?.gateway
                                ? { address: probe.earOf.address, name: undefined, txt: {}, ssh: null, gatewayPort: centre.port, version: centre.version }
                                : null;
                        }
                        return { ...station, gatewayPort: probe.port, version: probe.version };
                    }),
                );
                // Центр на компьютере или сервере: его объявление говорит роль, порт и версию.
                const computers: Found[] = homes
                    .filter((home) => home.txt.role === 'centre')
                    .map((home) => ({
                        ...home,
                        name: home.txt.name || home.txt.home || home.name,
                        gatewayPort: Number(home.txt.port) || undefined,
                        version: home.txt.version ?? null,
                        computer: true,
                    }));
                const all = [...probed.filter((entry): entry is Found => entry !== null), ...computers];
                const unique = all.filter((entry, at) => all.findIndex((other) => other.address === entry.address) === at);
                if (alive) setWithGateway(unique);
            } catch {
                if (alive) setFound([]);
            }
        })();
        return () => {
            alive = false;
        };
    }, [enabled]);
    return { done: found !== null, found: found ?? [], withGateway, helperDeclined: declined, declineHelper: () => setDeclined(true) };
}

/**
 * Колонка для людей: имя, что ей дали, а без него — модель («Станция Лайт 2»), и только потом адрес. Метку прошивки
 * `YandexIOReceiver-<серийный>` поиск именем не считает (gateway/src/stations/connect/discovery.rs; первые шаги 24.11).
 */
const nameOf = (station: FoundStation): string =>
    station.name ?? (station.txt?.platform ? drawingOf(null, station.txt.platform).title : station.address);

// ─── Схема «Как это будет» ─────────────────────────────────────

/** Карточка схемы: лайм — своё дома, янтарь — через интернет, пунктир — не нужно. */
interface MapNode {
    x: number;
    y: number;
    w: number;
    title: string;
    sub: string;
    tone: 'own' | 'out' | 'dim';
}

type MapLink = [number, number, number, number, boolean, string];

/** Схема доски: дом и облако, четыре карточки, связи, точки бегут по связям, ДиВи — где он живёт. */
export function PlaceMap({ nodes, links, badge, home, label }: { nodes: MapNode[]; links: MapLink[]; badge: { x: number; y: number }; home: string; label: string }) {
    const m = tr.onboarding.map;
    return (
        <svg class="map" viewBox="0 0 360 244" role="img" aria-label={label}>
            <rect class="zone-home" x="4" y="22" width="214" height="200" rx="20" />
            <rect class="zone-cloud" x="228" y="22" width="128" height="200" rx="20" />
            <text class="zt" x="16" y="42">
                {m.home}
            </text>
            <text class="zs" x="16" y="55">
                {home}
            </text>
            <text class="zt" x="292" y="42" text-anchor="middle">
                {m.cloud}
            </text>
            <text class="zs" x="292" y="55" text-anchor="middle">
                {m.viaInternet}
            </text>
            {links.map(([x1, y1, x2, y2, own]) => (
                <line key={`${x1}-${y1}-${x2}-${y2}`} class={own ? 'ln-own' : 'ln-net'} x1={x1} y1={y1} x2={x2} y2={y2} />
            ))}
            {nodes.map((node) => (
                <g key={node.title}>
                    <rect class={node.tone === 'dim' ? 'nd-dim' : `nd nd-${node.tone}`} x={node.x} y={node.y} width={node.w} height="56" rx="15" />
                    <text class={node.tone === 'dim' ? 'nt-dim' : 'nt'} x={node.x + 10} y={node.y + 26}>
                        {node.title}
                    </text>
                    <text class={node.tone === 'dim' ? 'ns-dim' : 'ns'} x={node.x + 10} y={node.y + 42}>
                        {node.sub}
                    </text>
                </g>
            ))}
            {links.map(([x1, y1, x2, y2, own, run]) => (
                <circle key={run} class={`${own ? 'pk' : 'pk-out'} ${run}`} cx={(x1 + x2) / 2} cy={(y1 + y2) / 2} r="4" />
            ))}
            <circle class="badge-ring" cx={badge.x} cy={badge.y} r="13" />
            <svg x={badge.x - 11} y={badge.y - 11} width="22" height="22" viewBox="0 0 256 256" aria-hidden="true">
                <path class="divi-body" d="M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z" />
                <path class="divi-eye" d="M98 110V138M158 110V138" />
            </svg>
        </svg>
    );
}

function choiceMap(choice: Choice): { nodes: MapNode[]; links: MapLink[]; badge: { x: number; y: number }; home: string } {
    const m = tr.onboarding.map;
    const pcOnly = choice === 'computer';
    const stationOnly = choice === 'station';
    const nodes: MapNode[] = [
        { x: 36, y: 60, w: 150, title: m.station, sub: pcOnly ? m.notNeeded : m.stationSub, tone: pcOnly ? 'dim' : 'own' },
        { x: 36, y: 148, w: 150, title: m.computer, sub: pcOnly ? m.computerSelf : stationOnly ? m.computerOff : m.computerHelps, tone: stationOnly ? 'dim' : 'own' },
        { x: 232, y: 60, w: 120, title: m.models, sub: m.modelsSub, tone: 'out' },
        { x: 232, y: 148, w: 120, title: m.alice, sub: pcOnly ? m.notNeeded : stationOnly ? m.aliceVoice : m.aliceMusic, tone: pcOnly ? 'dim' : 'out' },
    ];
    const links: MapLink[] = pcOnly
        ? [[111, 176, 292, 88, false, 'pk-cm']]
        : stationOnly
          ? [
                [111, 88, 292, 88, false, 'pk-sm'],
                [111, 88, 292, 176, false, 'pk-ss'],
            ]
          : [
                [111, 88, 111, 176, true, 'pk-sc'],
                [111, 88, 292, 88, false, 'pk-sm'],
                [111, 88, 292, 176, false, 'pk-ss'],
            ];
    const badge = pcOnly ? { x: 176, y: 148 } : { x: 176, y: 60 };
    const home = pcOnly ? m.homeComputer : stationOnly ? m.homeStation : m.homeBoth;
    return { nodes, links, badge, home };
}

// ─── «Где будет жить DuoWave» (доска Where) ────────────────────

function WhereStep({ choice, scan, onPick }: { choice: Choice; scan: Scan; onPick: (choice: Choice) => void }) {
    const t = tr.onboarding;
    const first = scan.found[0];
    const lines = choice === 'computer' ? t.linesComputer : choice === 'station' ? t.linesStation : t.linesStationPc;
    // Последняя строка «Только компьютер» и «Только колонка» — чего не будет (янтарь), у остальных — галочка.
    const warnLast = choice !== 'stationPc';
    // «Это новый дом»: колонки в другом доме не берём; других нет — выбор только «этот компьютер».
    const elsewhere = new Set(scan.helperDeclined ? scan.withGateway.map((station) => station.address) : []);
    const noFree = elsewhere.size > 0 && scan.found.every((station) => elsewhere.has(station.address));
    const option = (id: Choice, title: string, sub: string, icon: preact.ComponentChildren, wide = false, chip?: string) => (
        <button
            type="button"
            class={`opt${choice === id ? ' on' : ''}`}
            aria-pressed={choice === id}
            disabled={noFree && id !== 'computer'}
            onClick={() => onPick(id)}
        >
            <span class="radio" />
            <span class="opt-body">
                <span class="opt-title">
                    {title}
                    {chip && <span class="chip chip-lime">{chip}</span>}
                </span>
                <span class="opt-sub">{sub}</span>
            </span>
            <span class="opt-ico" style={wide ? { width: '56px' } : undefined}>
                {icon}
            </span>
        </button>
    );
    const map = choiceMap(choice);
    return (
        <div class="onb-split">
            <section class="onb-col onb-col-pick" style={{ maxWidth: '520px', gap: '14px' }}>
                <h1 class="h2">{t.whereTitle}</h1>
                <p class="lead" style={{ fontSize: '15px', lineHeight: 1.55 }}>
                    {t.whereAbout}
                </p>
                {!scan.done ? (
                    <div class="scan">
                        <span class="dot dot-busy" />
                        <span>{t.searching}</span>
                    </div>
                ) : first ? (
                    <div class="scan">
                        <span class="dot dot-ok" />
                        <span>
                            {t.foundIn} <b>{nameOf(first)}</b> · {first.address}
                        </span>
                    </div>
                ) : (
                    <div class="scan">
                        <span class="dot" />
                        <span>{t.noStationShort}</span>
                    </div>
                )}
                <div class="onb-opts">
                    {option(
                        'stationPc',
                        t.choiceStationPc,
                        noFree ? t.stationsElsewhere : t.choiceStationPcSub,
                        <>
                            <Ico name="speaker" />
                            <Ico name="laptop" />
                        </>,
                        true,
                        t.recommended,
                    )}
                    {option('station', t.choiceStation, noFree ? t.stationsElsewhere : t.choiceStationSub, <Ico name="speakerOnly" size="lg" />)}
                    {/* Своим домом — последним, только явным выбором (холст «Онбординг», доска Where, 21.11). */}
                    {option('computer', t.choiceComputer, t.choiceComputerSub, <Ico name="laptop" size="lg" />)}
                </div>
                <HomeFound scan={scan} choice={choice} />
                {choice !== 'computer' && (
                    <p class="small" style={{ margin: 0 }}>
                        {t.rootNote}{' '}
                        <a href={WIKI_ROOT} target="_blank" rel="noreferrer">
                            {t.rootWiki}
                        </a>
                    </p>
                )}
            </section>

            <section class="card onb-col" style={{ maxWidth: '560px', gap: '12px', padding: '22px 24px' }}>
                <div class="onb-row-between">
                    <h2 class="h3">{t.howItWill}</h2>
                    <div class="legend">
                        <span>
                            <i class="lg-own" />
                            {t.map.legendHome}
                        </span>
                        <span>
                            <i class="lg-net" />
                            {t.map.legendNet}
                        </span>
                    </div>
                </div>
                <PlaceMap {...map} label={`${t.map.home}: ${map.home}`} />
                <ul class="checks">
                    {lines.map((line, at) => {
                        const warn = warnLast && at === lines.length - 1;
                        return (
                            <li key={line}>
                                <span class={`mark ${warn ? 'mark-warn' : 'mark-ok'}`}>
                                    <Ico name={warn ? 'warn' : 'check'} size="sm" />
                                </span>
                                <span class="s">{line}</span>
                            </li>
                        );
                    })}
                </ul>
                <p class="small" style={{ margin: 0 }}>
                    {choice === 'computer' ? t.changeLaterComputer : t.changeLater}
                </p>
            </section>
        </div>
    );
}

/**
 * «Что есть в доме» (docs/plans/home-and-devices.md, этап 6): найденные устройства с ролями — колонка, на которой центр
 * уже работает, колонка, что станет центром, этот компьютер — Турбо или центр, пока он один; невозможное — с причиной
 * («нужен root»: SSH на колонке закрыт).
 */
function HomeFound({ scan, choice }: { scan: Scan; choice: Choice }) {
    const t = tr.onboarding;
    if (!scan.done) return null;
    const centre = new Set(scan.withGateway.map((station) => station.address));
    // Несколько колонок (решение 21.11): первая ставится центром, остальные «Ваш дом» добавит ушами потом.
    let centreTaken = false;
    const rows = scan.found.slice(0, 4).map((station) => {
        const busy = centre.has(station.address);
        // «Это новый дом»: центр на колонке — чужой дом, его не трогаем (этап 0 плана home-setup-flow).
        if (busy && scan.helperDeclined) return { key: station.address, name: nameOf(station), role: t.deviceOtherHome, warn: true };
        if (station.ssh === null && !busy) return { key: station.address, name: nameOf(station), role: t.needsRoot, warn: true };
        const role = busy ? t.deviceCentreHere : centreTaken ? t.deviceEarLater : t.deviceCanCentre;
        centreTaken = true;
        return { key: station.address, name: nameOf(station), role, warn: false };
    });
    rows.push({
        key: 'this-computer',
        name: t.deviceThisComputer,
        role: choice === 'computer' ? t.deviceCentreAlone : t.deviceTurbo,
        warn: false,
    });
    return (
        <div class="onb-found" aria-label={t.yourDevices}>
            <span class="small">{t.yourDevices}</span>
            <ul class="checks">
                {rows.map((row) => (
                    <li key={row.key}>
                        <span class={`mark ${row.warn ? 'mark-warn' : 'mark-ok'}`}>
                            <Ico name={row.warn ? 'warn' : 'check'} size="sm" />
                        </span>
                        <span class="s">
                            <b>{row.name}</b> — {row.role}
                        </span>
                    </li>
                ))}
            </ul>
        </div>
    );
}

// ─── «На колонке уже работает DuoWave» (доска Helper) ──────────

function HelperStep({
    found,
    onNo,
    onLinked,
}: {
    found: Found;
    onNo: () => void;
    onLinked: (station: { key: string; name: string; computer?: boolean }) => void;
}) {
    const t = tr.onboarding;
    const m = t.map;
    const name = nameOf(found);
    const port = found.gatewayPort ?? 8790;
    // Код вместо пароля пульта (этапы 2–3 плана home-setup-flow): компьютер просит дом, центр говорит код. Колонка со
    // старым DuoWave кода не знает — тогда пароль её пульта, как раньше.
    const [mode, setMode] = useState<'code' | 'password'>('code');
    const [asked, setAsked] = useState<PairCodeAsked | null>(null);
    const [look, setLook] = useState<PairCodeLook | null>(null);
    const [code, setCode] = useState('');
    const [password, setPassword] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Центр-компьютер связывается с этим компьютером сам (ключ подключения): своей колонки тут нет — шаги закончены.
    const linked = (view: StationGatewayView | { helper?: boolean; key?: string }) =>
        onLinked({ key: 'key' in view && view.key ? view.key : '', name, computer: 'helper' in view && !!view.helper });
    const askNow = async (): Promise<void> => {
        setError(null);
        setWrong(null);
        setCodeShy(false);
        setLook(null);
        setCode('');
        try {
            setAsked(await api.pairCode(found.address, port));
        } catch (e) {
            setAsked(null);
            setError(problemOf(e));
        }
    };
    useEffect(() => {
        if (mode === 'code' && !asked) void askNow();
    }, [mode]);
    // Пока ждём: «Разрешить» в пульте дома впускает без кода — компьютер узнаёт об этом сам.
    useEffect(() => {
        if (!asked || mode !== 'code') return;
        let alive = true;
        const timer = window.setInterval(() => {
            void api
                .pairCodeLook(asked.id)
                .then((fresh) => {
                    if (!alive) return;
                    setLook(fresh);
                    if (fresh.joined && fresh.station) linked(fresh.station);
                })
                .catch(() => undefined);
        }, 2500);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, [asked?.id, mode]);
    const digits = code.replace(/\D/g, '');
    const closed = !!look && (look.status === 'denied' || look.status === 'closed');
    /**
     * «Подключить к дому» с пустым или неполным кодом (замечание 13 после 0.18.1: кнопка была выключена, и нажатие
     * молчало): поле подсвечено, под ним — что ввести и где этот код взять. Начали вводить снова — подсветка уходит.
     */
    const [codeShy, setCodeShy] = useState(false);
    /**
     * Код не подошёл (замечание 13: неверный код тоже молчал — ответ дома был мелко внизу, а поле пустело): ответ — под
     * полем, поле подсвечено, введённое остаётся выделенным, чтобы поправить цифру. Третий неверный закрывает запрос —
     * тогда строка «закрыт» и что делать дальше.
     */
    const [wrong, setWrong] = useState<string | null>(null);
    const codeWhere = !asked
        ? ''
        : asked.told?.spoken
          ? t.helperCodeWhereSpoken(name)
          : asked.told?.window
            ? t.helperCodeWhereWindow
            : asked.told?.printed
              ? t.helperCodeWherePrinted
              : t.helperCodeWhereSilent;
    const enter = async (): Promise<void> => {
        if (!asked || busy) return;
        if (digits.length !== 6) {
            setCodeShy(true);
            document.getElementById('onb-code')?.focus();
            return;
        }
        setBusy(true);
        setError(null);
        try {
            linked(await api.pairCodeEnter(asked.id, digits));
        } catch (e) {
            const fresh = await api.pairCodeLook(asked.id).catch(() => null);
            if (fresh) setLook(fresh);
            if (e instanceof ApiError && e.status === 403) {
                // Закрыт третьим неверным — это скажет строка «закрыт»; иначе — сколько попыток осталось.
                if (fresh?.status !== 'closed' && fresh?.status !== 'denied')
                    setWrong(fresh?.status === 'pending' ? t.helperCodeWrong(fresh.triesLeft) : problemOf(e));
            } else setError(problemOf(e));
            window.setTimeout(() => (document.getElementById('onb-code') as HTMLInputElement | null)?.select(), 0);
        } finally {
            setBusy(false);
        }
    };
    const sayAgain = async (): Promise<void> => {
        if (!asked) return;
        await api.pairCodeSay(asked.id).catch((e) => setError(problemOf(e)));
    };
    const byPassword = async (): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            linked(await api.pairStation(found.address, port, password, ''));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const lead = !asked
        ? t.helperAsking
        : asked.told?.spoken
          ? t.helperCodeLead(name)
          : asked.told?.window
            ? t.helperCodeWindow
            : asked.told?.printed
              ? t.helperCodePrinted
              : t.helperCodeSilent(name);
    return (
        <div class="onb-split">
            <section class="onb-col" style={{ maxWidth: '540px', gap: '16px' }}>
                <div class="scan" style={{ alignSelf: 'flex-start' }}>
                    <span class="dot dot-ok" />
                    <span>
                        {t.helperFound} <b>«{name}»</b>
                        {found.version ? ` · ${t.helperVersion(found.version)}` : ''}
                    </span>
                </div>
                <h1 class="h2">{found.computer ? t.helperTitleComputer(name) : t.helperTitle(name)}</h1>
                <p class="lead" style={{ fontSize: '15px', lineHeight: 1.55 }}>
                    {t.helperAbout}
                </p>
                <div class="card card-tight" style={{ padding: '4px 18px' }}>
                    <ul class="checks">
                        {(
                            [
                                ['wave', t.helperVoice, t.helperVoiceSub],
                                ['mic', t.helperEar, t.helperEarSub],
                                ['spark', t.helperHeavy, t.helperHeavySub],
                            ] as const
                        ).map(([icon, title, sub]) => (
                            <li key={title}>
                                <span class="mark mark-ok">
                                    <Ico name={icon} size="sm" />
                                </span>
                                <span>
                                    <span class="t">{title}</span>
                                    <br />
                                    <span class="s">{sub}</span>
                                </span>
                            </li>
                        ))}
                    </ul>
                </div>
                <p class="small" style={{ margin: 0 }}>
                    {t.helperStays}
                </p>
            </section>

            <section class="card onb-col" style={{ maxWidth: '480px', gap: '14px', padding: '24px' }}>
                <PlaceMap
                    nodes={[
                        { x: 36, y: 60, w: 150, title: m.stationNamed(name), sub: m.stationMain, tone: 'own' },
                        { x: 36, y: 148, w: 150, title: m.thisComputer, sub: m.helper, tone: 'own' },
                        { x: 232, y: 60, w: 120, title: m.models, sub: m.modelsSub, tone: 'out' },
                        { x: 232, y: 148, w: 120, title: m.alice, sub: m.aliceMusic, tone: 'out' },
                    ]}
                    links={[
                        [111, 88, 111, 176, true, 'pk-sc'],
                        [111, 88, 292, 88, false, 'pk-sm'],
                    ]}
                    badge={{ x: 176, y: 60 }}
                    home={m.homeHelper}
                    label={t.helperTitle(name)}
                />
                {mode === 'code' ? (
                    <>
                        <p class="small" role="status" style={{ margin: 0 }}>
                            {closed ? (look?.why ?? t.helperAskAgain) : lead}
                        </p>
                        {closed ? (
                            <>
                                <button type="button" class="dw-btn dw-btn-light" onClick={() => void askNow()}>
                                    {t.helperAskAgain}
                                </button>
                                <p class="small" style={{ margin: 0 }}>
                                    {t.helperClosedNext}
                                </p>
                            </>
                        ) : (
                            <div class="field">
                                <label for="onb-code">{t.helperCodeLabel}</label>
                                {/* DESIGN.md «number»: 22 px, цифры одной ширины. */}
                                <input
                                    id="onb-code"
                                    name="one-time-code"
                                    class="input"
                                    inputMode="numeric"
                                    autocomplete="one-time-code"
                                    maxLength={8}
                                    placeholder="•• •• ••"
                                    disabled={!asked || busy}
                                    value={code}
                                    aria-invalid={codeShy || !!wrong || undefined}
                                    aria-describedby={codeShy || wrong ? 'onb-code-shy' : undefined}
                                    style={{ fontSize: '22px', lineHeight: '26px', letterSpacing: '0.2em', fontVariantNumeric: 'tabular-nums' }}
                                    onInput={(event) => {
                                        setCode((event.target as HTMLInputElement).value);
                                        setCodeShy(false);
                                        setWrong(null);
                                    }}
                                    onKeyDown={(event) => event.key === 'Enter' && void enter()}
                                />
                                {(codeShy || wrong) && (
                                    <span id="onb-code-shy" class="small onb-code-shy" role="alert">
                                        {wrong ?? (digits.length ? t.helperCodeShort(digits.length) : t.helperCodeEmpty)} {codeWhere}
                                    </span>
                                )}
                                <span class="small">
                                    {/* После неверного кода — сколько попыток осталось (ответ дома: «Код не подошёл. Осталось попыток: 2.»). */}
                                    {look && look.triesLeft < 3 ? t.helperCodeTries(look.triesLeft) : t.helperCodeHint}
                                    {look ? ` · ${t.helperWaiting}` : ''}
                                </span>
                            </div>
                        )}
                        <div class="onb-row-between" style={{ flexWrap: 'wrap', gap: '8px' }}>
                            <button type="button" class="btn-ghost" style={{ padding: 0 }} disabled={!asked || closed} onClick={() => void sayAgain()}>
                                {t.helperSayAgain}
                            </button>
                            <button type="button" class="btn-ghost" style={{ padding: 0 }} onClick={() => (setError(null), setMode('password'))}>
                                {t.helperByPassword}
                            </button>
                        </div>
                        <p class="small" style={{ margin: 0 }}>
                            {t.helperNotHeard}
                        </p>
                    </>
                ) : (
                    <>
                        <div class="field">
                            <label for="onb-stpw">{t.helperPassword(name)}</label>
                            {/* Пароль другого устройства — пульта колонки, не входа сюда: менеджер паролей не должен ни
                                подставлять пароль этого компьютера, ни сохранять этот вместо него (как SecretField на столе). */}
                            <input
                                id="onb-stpw"
                                name="speaker-password"
                                class="input"
                                type="password"
                                placeholder={t.helperPasswordHint}
                                autocomplete="off"
                                value={password}
                                onInput={(event) => setPassword((event.target as HTMLInputElement).value)}
                                onKeyDown={(event) => event.key === 'Enter' && password && void byPassword()}
                            />
                        </div>
                        <button type="button" class="btn-ghost" style={{ padding: 0, alignSelf: 'flex-start' }} onClick={() => (setError(null), setMode('code'))}>
                            {t.helperByCode}
                        </button>
                    </>
                )}
                {error && (
                    <p class="small" role="alert" style={{ margin: 0, color: 'var(--bad)' }}>
                        {error}
                    </p>
                )}
                <p class="small" style={{ margin: 0 }}>
                    {t.helperLinkNote}
                </p>
                <div class="onb-row-between">
                    <button type="button" class="btn-ghost" style={{ padding: 0 }} onClick={onNo}>
                        {t.helperNo}
                    </button>
                    <button
                        type="button"
                        class="dw-btn dw-btn-lg dw-btn-accent"
                        disabled={busy || (mode === 'code' ? !asked || closed : !password)}
                        aria-busy={busy || undefined}
                        onClick={() => void (mode === 'code' ? enter() : byPassword())}
                    >
                        {t.helperYes}
                    </button>
                </div>
            </section>
        </div>
    );
}
