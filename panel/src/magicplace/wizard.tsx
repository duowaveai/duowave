// Мастер установки (проверка начисто 18.11, п. 15; владелец выбрал вариант А «Один мастер»): «Установить» в MagicPlace
// открывает одно окно — что поставится (сам пакет, его пары вроде слова «Джарвис», голос персонажа на компьютере) и
// какие права, одна кнопка «Поставить всё». Дальше в том же окне шаги с галочками: пакеты, персонаж — голос дома, его
// голос на компьютере, приложения запускаются. В конце — что сказать: «Джарвис, как дела?». Права уходят шлюзу с
// установкой — потом приложение их не спрашивает. Окно закрыли посреди — установка помнится (wizard-state.ts), карточка
// приложения предлагает «Продолжить установку», и мастер показывает, где остановились: каждый шаг сначала смотрит, не
// сделан ли он уже.

import { useEffect, useRef, useState } from 'preact/hooks';

import { CatalogEntry, HomeVoicePlan, InstallApi, installApi, problemOf } from '../api';
import { IconCheck } from '../icons';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { Sheet, WinRow, WinSection } from '../ui/sheet';
import { PackageThumb, busRights, isInstalled, placeOf, rightsLines } from '../screens/catalog';
import { EnginePicker, defaultEngine, engineStatus, fitsDisk, noRoom, planGb as gb, usableEngines as usable } from '../ui/voice-plan';
import { openTarget } from './section';
import {
    WizardOpening,
    installProgress,
    markUnfinished,
    onInstallProgress,
    onInstallWizard,
    onShowInstallWizard,
    setInstallProgress,
    showInstallWizard,
} from './wizard-state';
import { usePendingInstall } from './pending-install';
import { settleVoice, settlingLine, showSettling, useSettling } from './settling';
import { VoiceWaitingPill, useVoiceAttention } from '../voice/pending-voice';
import { isCallWord } from './filters';

/**
 * Как пользоваться приложением, которое своих фраз не даёт (п. 71: AirPlay кончался «Скажите: «Санчес, как дела?»»):
 * по id пакета. VPN зовётся голосом через «Мозг» — `vpn_on` даёт шлюз, пока приложение стоит (gateway/src/egress/
 * tools.rs); AirPlay — не голосом, а выбором колонки на телефоне. Своего поля в манифесте для этого пока нет — когда
 * появится (`usage`, `examples`), брать оттуда.
 */
// Ключи — id пакетов каталога: AirPlay, VPN.
const USAGE: Record<string, (place: { station: string | null; word: string }) => string> = {
    'p-4db294c19645': ({ station }) => (station ? tr.wizard.useAirplay(station) : tr.wizard.useAirplayAny),
    'p-2a4594f148e9': ({ word }) => tr.wizard.sayPhrase(word, tr.wizard.vpnPhrase),
};
import './magicplace.css';

type StepState = 'wait' | 'run' | 'done' | 'fail' | 'skip' | 'later';

interface Step {
    key: string;
    title: string;
    state: StepState;
    note?: string;
}

/** Что шаг сделал: состояние и подпись. */
type Outcome = { state: Exclude<StepState, 'wait' | 'run'>; note?: string };

interface StepPlan {
    key: string;
    title: string;
    run: (show: (note: string) => void) => Promise<Outcome>;
}

const sleep = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));
/**
 * Голос на компьютере ставится долго (модель — сотни МБ) или не ставится вовсе (у компьютера нет модели для этого
 * персонажа): мастер ждёт полторы минуты и дальше не держит человека — шаг «догонит», в конце «Почти готово»
 * (проверка начисто 18.11, п. 22: ждал 20 минут и падал по времени).
 */
const VOICE_WAIT_MS = 90 * 1000;
const APP_WAIT_MS = 90 * 1000;
/** Модель и голос на Турбо качаются долго, но с ходом: мастер ждёт, пока ход идёт; стоит без движения столько — «догонит». */
const STILL_MS = 120 * 1000;

/** Слово приложения-активатора из его названия: «Слово «Джарвис» (демо)» → «Джарвис». */
function wordOf(entry: CatalogEntry): string | null {
    if (!(entry.permissions ?? []).includes('call:system/activator')) return null;
    return /«([^»]+)»/.exec(entry.title)?.[1] ?? null;
}

/** Имя персонажа для людей: без пометок в скобках («Джарвис (демо)» → «Джарвис»). */
const plainName = (title: string): string => title.replace(/\s*\([^)]*\)\s*$/, '').trim() || title;

const runsCode = (entry: CatalogEntry): boolean => entry.kind === 'app' || entry.kind === 'plugin';

function sizeOf(entry: CatalogEntry): string {
    const place = placeOf(entry.kind, entry.runtime, entry.runs);
    return entry.sizeMb ? `${place} · ${tr.wizard.sizeMb(entry.sizeMb)}` : place;
}

/** Все пакеты установки по порядку: сам пакет и его пары, которых ещё нет и которые можно поставить. */
function partsOf(entry: CatalogEntry, all: CatalogEntry[]): CatalogEntry[] {
    const pairs = (entry.companions ?? [])
        .map((pair) => all.find((item) => item.id === pair.id))
        .filter((item): item is CatalogEntry => !!item && (isInstalled(item) || item.state === 'available'));
    return [entry, ...pairs];
}

/**
 * Голос колонки — компьютера, у центра (п. 56). Центр не прочитался или не переключился — шаг падает с причиной;
 * компьютер голос не даёт — «позже» со словами, что включить; переключено или уже так — null, шаг идёт дальше.
 */
async function toComputerVoice(api: InstallApi): Promise<Outcome | null> {
    const t = tr.wizard;
    let home;
    try {
        home = await api.homeVoice();
    } catch (e) {
        return { state: 'fail', note: t.voiceSwitchFailed(problemOf(e)) };
    }
    if (!home.offered) return { state: 'later', note: t.voiceNotOffered };
    if (home.chosen === 'computer') return null;
    try {
        await api.setHomeVoice('computer');
    } catch (e) {
        return { state: 'fail', note: t.voiceSwitchFailed(problemOf(e)) };
    }
    return null;
}

/** Ждать, пока центр скажет: голос — компьютера, и он на связи (`chosen: computer`, `up`). Не дождался — «позже». */
async function centreSpeaksComputer(api: InstallApi, show: (note: string) => void): Promise<Outcome> {
    const t = tr.wizard;
    const until = Date.now() + STILL_MS;
    for (;;) {
        const home = await api.homeVoice().catch(() => null);
        if (home?.chosen === 'computer' && home.up) return { state: 'done', note: t.voiceOnDone };
        if (home && home.offered && home.chosen !== 'computer') {
            const switched = await toComputerVoice(api);
            if (switched) return switched;
        }
        show(t.voiceSwitching);
        if (Date.now() > until) return { state: 'later', note: t.voiceNotSwitched };
        await sleep(3000);
    }
}

function plan(
    api: InstallApi,
    entry: CatalogEntry,
    parts: CatalogEntry[],
    onChanged?: () => void,
    voice?: { plan: HomeVoicePlan; engine: string | null },
): StepPlan[] {
    const t = tr.wizard;
    const steps: StepPlan[] = parts.map((part) => ({
        key: `pkg:${part.id}`,
        title: t.stepPackage(t.quote(part.title)),
        run: async () => {
            const fresh = (await api.packagesCatalog()).packages.find((item) => item.id === part.id);
            if (!fresh || !isInstalled(fresh)) {
                await api.installPackage(part.id, part.version, true, busRights(part).length > 0);
                onChanged?.();
            }
            return { state: 'done', note: t.donePackage };
        },
    }));
    if (entry.kind === 'character') {
        const name = plainName(entry.title);
        // Модель не помещается на диск компьютера (п. 40): голос не ставим вовсе — персонаж говорит запасным.
        const crowded = !!voice?.plan.turbo && noRoom(voice.plan);
        steps.push({
            key: 'character',
            title: t.stepCharacter(name),
            run: async () => {
                const view = await api.characters();
                const character = view.characters.find((item) => item.package?.id === entry.id);
                if (!character) throw new Error(t.failedTimeout);
                await api.chooseCharacter(character.id, { install: !crowded });
                return { state: 'done', note: t.doneCharacter };
            },
        });
        // Турбо отдал план голоса (п. 40): модель на компьютере с её ходом, потом голос персонажа под неё.
        const turbo = voice?.plan.turbo;
        const chosen = voice?.plan.plan?.engines.find((engine) => engine.id === voice.engine);
        if (crowded)
            steps.push({ key: 'voice', title: t.stepVoice, run: async () => ({ state: 'skip', note: t.voiceNoRoom(voice?.plan.plan?.fallback?.title ?? null) }) });
        else if (turbo && voice?.plan.plan && chosen) {
            const computer = voice.plan.plan.computer ?? turbo.name;
            const read = () => api.homeVoicePlan({ package: entry.id }).catch(() => null);
            const fallback = voice.plan.plan.fallback?.title ?? null;
            /** Ждать, пока ход идёт; без движения STILL_MS — «догонит». */
            const follow = async (show: (note: string) => void, done: (fresh: HomeVoicePlan) => boolean, note: (fresh: HomeVoicePlan) => string | null): Promise<Outcome> => {
                let last = '';
                let moved = Date.now();
                for (;;) {
                    const fresh = await read();
                    if (fresh && !fresh.turbo?.up) return { state: 'later', note: t.turboAway(computer) };
                    if (fresh && done(fresh)) return { state: 'done' };
                    const now = (fresh && note(fresh)) ?? '';
                    if (now !== last) {
                        last = now;
                        moved = Date.now();
                    }
                    show(now ? `${now} · ${t.meanwhile(fallback)}` : t.meanwhile(fallback));
                    if (Date.now() - moved > STILL_MS) return { state: 'later', note: t.laterVoice };
                    await sleep(3000);
                }
            };
            if (chosen.state !== 'ready')
                steps.push({
                    key: 'model',
                    title: t.stepModel(chosen.title, computer),
                    run: async (show) => {
                        const outcome = await follow(
                            show,
                            (fresh) => fresh.plan?.engines.find((engine) => engine.id === chosen.id)?.state === 'ready',
                            (fresh) => {
                                const going = fresh.plan?.progress;
                                if (going?.stage !== 'model' || going.engine !== chosen.id) return null;
                                // Слова шлюза — мегабайты, скорость, «стояло — повторяю» (п. 66); старый шлюз — по процентам.
                                if (going.detail) return `${t.modelPercent(going.percent ?? 0)} · ${going.detail}`;
                                return going.totalMb ? t.modelProgress(going.percent ?? 0, gb(going.doneMb), gb(going.totalMb)) : t.modelPercent(going.percent ?? 0);
                            },
                        );
                        return outcome.state === 'done' ? { state: 'done', note: t.engineReady } : outcome;
                    },
                });
            steps.push({
                key: 'voice',
                title: t.stepVoiceOn(name, computer),
                run: async (show) => {
                    // Колонка говорит голосом компьютера — его голос и есть голос персонажа. Переключает центр (у
                    // помощника — через проход, `api` мастера), и ошибка — словами (п. 56: молчаливый catch оставил
                    // колонку на Алисе, хотя голос Рика на компьютере встал).
                    const switched = await toComputerVoice(api);
                    if (switched) return switched;
                    const outcome = await follow(
                        show,
                        (fresh) => !!fresh.plan?.voiceOn,
                        (fresh) => {
                            const going = fresh.plan?.progress;
                            return going?.stage === 'voice' ? t.modelPercent(going.percent ?? 0) : null;
                        },
                    );
                    if (outcome.state !== 'done') return outcome;
                    // Сделано — когда центр говорит голосом компьютера, а не когда голос встал на Турбо.
                    return centreSpeaksComputer(api, show);
                },
            });
        } else steps.push({
            key: 'voice',
            title: t.stepVoice,
            run: async (show) => {
                const until = Date.now() + VOICE_WAIT_MS;
                let asked = false;
                for (;;) {
                    // На компьютере без колонки окна выбора голоса нет: голос там ставит сам выбор персонажа.
                    const voice = await api.homeVoice().catch(() => null);
                    if (!voice || (!voice.offered && !voice.cloud)) return { state: 'skip', note: t.skipVoiceNoComputer };
                    if (voice.offered && voice.chosen !== 'computer' && !asked) {
                        asked = true;
                        try {
                            await api.setHomeVoice('computer');
                        } catch (e) {
                            return { state: 'fail', note: t.voiceSwitchFailed(problemOf(e)) };
                        }
                        continue;
                    }
                    if (voice.up) return { state: 'done', note: t.doneVoice(voice.voice ?? null) };
                    if (voice.reached === false) return { state: 'later', note: t.waitingVoiceAway };
                    show(t.runningVoice);
                    if (Date.now() > until) return { state: 'later', note: t.laterVoice };
                    await sleep(3000);
                }
            },
        });
    }
    for (const part of parts.filter(runsCode)) {
        steps.push({
            key: `app:${part.id}`,
            title: t.stepApp(t.quote(part.title)),
            run: async (show) => {
                const until = Date.now() + APP_WAIT_MS;
                show(t.runningApp);
                for (;;) {
                    const view = await api.packages().catch(() => null);
                    const here = view?.packages.find((item) => item.id === part.id);
                    if (here?.enabled && (here.state === 'loaded' || here.state === 'ready')) return { state: 'done', note: t.doneApp };
                    if (here?.problem) return { state: 'fail', note: here.problem };
                    if (here && !here.enabled) return { state: 'fail', note: t.failedApp };
                    if (Date.now() > until) return { state: 'fail', note: t.failedTimeout };
                    await sleep(2000);
                }
            },
        });
    }
    return steps;
}

/** Мастер — один на пульт, в app.tsx рядом с листом «да». */
export function InstallWizard() {
    const [opening, setOpening] = useState<WizardOpening | null>(null);
    useEffect(
        () =>
            onInstallWizard((next) => {
                setOpening((old) => {
                    old?.done(false);
                    return next;
                });
            }),
        [],
    );
    if (!opening) return null;
    return <Wizard key={opening.entry.id} opening={opening} onClose={() => setOpening(null)} />;
}

function Wizard({ opening, onClose }: { opening: WizardOpening; onClose: () => void }) {
    const t = tr.wizard;
    const panel = usePanel();
    const { entry } = opening;
    /** Свой шлюз или центр дома через проход помощника (первые шаги на компьютере, п. 42). */
    const api = installApi(opening.station);
    const [parts, setParts] = useState<CatalogEntry[]>([entry]);
    /** Пары прочитаны из каталога (или не прочитались): продолжать начатое можно только после этого. */
    const [read, setRead] = useState(false);
    const [phase, setPhase] = useState<'ask' | 'run' | 'end'>(opening.resume ? 'run' : 'ask');
    const [steps, setSteps] = useState<Step[]>([]);
    const [failed, setFailed] = useState(false);
    /** «Свернуть»: окно спрятано, шаги идут дальше; пилюля у дока показывает ход и разворачивает (п. 26). */
    const [hidden, setHidden] = useState(!!opening.auto);
    /** Вопрос мастера показан: обычно сразу; выбор, сделанный раньше (`auto`), — только если вопрос правда есть. */
    const [asking, setAsking] = useState(!opening.auto);
    const installed = useRef(false);
    const alive = useRef(true);
    const plans = useRef<StepPlan[]>([]);
    /** План голоса персонажа от Турбо через центр (п. 40): undefined — читаю, null — нет (центр старый, Турбо нет). */
    const [voicePlan, setVoicePlan] = useState<HomeVoicePlan | null | undefined>(entry.kind === 'character' ? undefined : null);
    const [engine, setEngine] = useState<string | null>(null);
    const [words, setWords] = useState<string[]>([]);
    useEffect(() => {
        if (entry.kind !== 'character') return;
        void api
            .homeVoicePlan({ package: entry.id })
            .then((fresh) => {
                if (!alive.current) return;
                setVoicePlan(fresh);
                setEngine(defaultEngine(fresh));
            })
            .catch(() => alive.current && setVoicePlan(null));
    }, [entry.id]);

    useEffect(() => {
        alive.current = true;
        void api
            .packagesCatalog()
            .then((view) => {
                if (!alive.current) return;
                setParts(partsOf(view.packages.find((item) => item.id === entry.id) ?? entry, view.packages));
                // Слова, что уже стоят: персонаж без своего слова зовётся ими (п. 49) — слово работает для любого.
                setWords(view.packages.filter(isInstalled).map(wordOf).filter((word): word is string => !!word));
            })
            .catch(() => undefined)
            .finally(() => alive.current && setRead(true));
        return () => {
            alive.current = false;
        };
    }, [entry.id]);

    const close = (): void => {
        setInstallProgress(null);
        opening.done(installed.current);
        onClose();
    };

    useEffect(() => onShowInstallWizard(() => setHidden(false)), []);
    useEffect(() => {
        if (phase === 'ask') return;
        const done = steps.filter((step) => step.state !== 'wait' && step.state !== 'run').length;
        // Идёт модель с ходом — пилюля говорит «модель 37 %».
        const percent = /(\d+) %/.exec(steps.find((step) => step.state === 'run' && step.key === 'model')?.note ?? '')?.[1];
        setInstallProgress({
            title: entry.title,
            done,
            total: steps.length,
            finished: phase === 'end',
            hidden,
            detail: percent ? tr.wizard.pillModel(Number(percent)) : undefined,
            kind: entry.kind,
            id: entry.id,
            image: entry.preview?.image ? api.mediaPath(entry.preview.image) : null,
        });
    }, [steps, phase, hidden]);
    useEffect(() => () => setInstallProgress(null), []);

    /** Шаги по порядку, с того, что ещё не сделано: каждый сам смотрит, не сделан ли он уже. */
    const run = async (from = 0): Promise<void> => {
        setFailed(false);
        for (let at = from; at < plans.current.length; at++) {
            const step = plans.current[at];
            const set = (patch: Partial<Step>) => setSteps((all) => all.map((item) => (item.key === step.key ? { ...item, ...patch } : item)));
            set({ state: 'run', note: undefined });
            let outcome: Outcome;
            try {
                outcome = await step.run((note) => alive.current && set({ note }));
            } catch (e) {
                outcome = { state: 'fail', note: e instanceof Error && !('status' in e) ? e.message : problemOf(e) };
            }
            if (!alive.current) return;
            set(outcome);
            if (step.key === `pkg:${entry.id}` && outcome.state === 'done') installed.current = true;
            // Голос отпущен догонять самому (модель ещё скачивается): пилюля «донастраивается» живёт, пока он не
            // включится, — и после конца мастера (п. 67).
            if ((step.key === 'model' || step.key === 'voice') && outcome.state === 'later' && entry.kind === 'character')
                settleVoice(entry, opening.station);
            if (outcome.state === 'fail') {
                setFailed(true);
                return;
            }
        }
        markUnfinished(null);
        setPhase('end');
    };

    const start = (): void => {
        // Выбор модели — центру сразу: он помнит его по пакету и передаст Турбо, когда персонаж встанет.
        if (voicePlan?.plan && engine) void api.setHomeVoiceEngine({ package: entry.id }, engine).catch(() => undefined);
        plans.current = plan(api, entry, parts, opening.onChanged, voicePlan ? { plan: voicePlan, engine } : undefined);
        // Голос персонажа ставится на Турбо (модель — гигабайты): пилюля «донастраивается» живёт, пока он не включится, —
        // и когда мастер кончился, и после перезагрузки окна (уход в окно колонки после первых шагов, п. 69).
        if (entry.kind === 'character' && voicePlan?.turbo && voicePlan.plan && !voicePlan.plan.voiceOn && !noRoom(voicePlan))
            settleVoice(entry, opening.station);
        setSteps(plans.current.map((step) => ({ key: step.key, title: step.title, state: 'wait' })));
        markUnfinished(entry.id);
        setPhase('run');
        void run();
    };

    // Продолжить начатое: сразу шаги, когда пары прочитаны.
    const resumed = useRef(false);
    useEffect(() => {
        if (opening.resume && !resumed.current && read && voicePlan !== undefined) {
            resumed.current = true;
            start();
        }
    }, [read, voicePlan]);

    // Подсказка приложению в конце (п. 71): его своя фраза (`intents` пакета) со словом, что на деле зовёт; нет фраз —
    // как им пользоваться (USAGE); нет и этого — просто «Готово».
    const [appHint, setAppHint] = useState<string | null>(null);
    useEffect(() => {
        if (phase !== 'end' || entry.kind === 'character' || isCallWord(entry)) return;
        const calling = words[0] ?? t.alice;
        const station = panel.health?.placement?.name ?? (panel.role.role === 'helper' ? panel.role.station?.name : null) ?? null;
        void api
            .packages()
            .then((view) => view.packages.find((item) => item.id === entry.id)?.contributes?.phrases?.[0] ?? null)
            .catch(() => null)
            .then((phrase) => {
                if (!alive.current) return;
                if (phrase) return setAppHint(t.sayPhrase(calling, phrase.replace(/\{\w+\}/g, '…')));
                const usage = USAGE[entry.id];
                setAppHint(usage ? usage({ station, word: calling }) : null);
            });
    }, [phase]);

    // Выбор сделан раньше (п. 63): спрашивать нечего — шаги сразу, свёрнуто; есть что выбрать — обычный вопрос.
    const decided = useRef(false);
    useEffect(() => {
        if (!opening.auto || decided.current || !read || voicePlan === undefined) return;
        decided.current = true;
        // Вопрос есть, когда правда выбирать (две модели помещаются), места нет совсем или пакет просит прав — «Поставить
        // всё» и есть «да» на них; иначе спрашивать нечего.
        const ask =
            usable(voicePlan).filter(fitsDisk).length > 1 ||
            (entry.kind === 'character' && noRoom(voicePlan)) ||
            parts.flatMap(rightsLines).length > 0;
        if (ask) {
            setHidden(false);
            setAsking(true);
        } else start();
    }, [read, voicePlan]);

    const retry = (): void => {
        const at = steps.findIndex((step) => step.state === 'fail');
        if (at >= 0) void run(at);
    };

    const character = entry.kind === 'character';
    // Своё слово персонажа (его пара) или слово, что уже стоит: оно работает для любого персонажа.
    const own = parts.map(wordOf).find(Boolean) ?? null;
    const name = plainName(entry.title);
    const voiceNeed = (entry.needs ?? []).find((need) => need.kind === 'engine') ?? (entry.needs ?? [])[0];
    const choices = usable(voicePlan);
    const picked = choices.find((choice) => choice.id === engine);
    const computer = voicePlan?.plan?.computer ?? voicePlan?.turbo?.name ?? '';
    const rights = [...new Set(parts.flatMap(rightsLines))];

    if (phase === 'ask' && !asking) return null;
    if (phase === 'ask')
        return (
            <Sheet
                title={t.title(t.quote(entry.title))}
                about={t.lead}
                onClose={close}
                footer={
                    <>
                        <button class="btn btn-quiet" onClick={close}>
                            {t.cancel}
                        </button>
                        <button class="btn btn-main" disabled={!read || voicePlan === undefined} onClick={start}>
                            {t.start}
                        </button>
                    </>
                }
            >
                <WinSection title={t.what} group>
                    {parts.map((part, at) => (
                        <WinRow
                            key={part.id}
                            icon={<PackageThumb url={part.preview?.image ? api.mediaPath(part.preview.image) : null} />}
                            title={part.title}
                            sub={at > 0 ? (entry.companions?.find((pair) => pair.id === part.id)?.why ?? sizeOf(part)) : sizeOf(part)}
                        />
                    ))}
                    {character && picked && computer ? (
                        <WinRow title={t.itemVoiceOn(name, computer)} sub={`${picked.title} · ${engineStatus(picked)}`} />
                    ) : character && computer && noRoom(voicePlan) ? (
                        <WinRow title={t.itemVoiceOn(name, computer)} sub={t.voiceNoRoom(voicePlan?.plan?.fallback?.title ?? null)} />
                    ) : character ? (
                        <WinRow
                            title={t.itemVoice}
                            sub={voiceNeed?.downloadMb ? t.itemVoiceSize(voiceNeed.downloadMb) : undefined}
                        />
                    ) : null}
                </WinSection>
                {/* Выбор модели — только когда подходят две и больше (п. 40); место и ОЗУ — до «Поставить всё», не после. */}
                {character && <EnginePicker plan={voicePlan} engine={engine} onSelect={setEngine} />}
                {/* Своего слова у персонажа нет — честно, как его звать: прежним словом или «Алиса» (п. 49). */}
                {character && !own && <p class="win-note m-0">{t.callAsBefore([...words, tr.wizard.alice])}</p>}
                {rights.length > 0 && (
                    <WinSection title={t.rights}>
                        <ul class="wz-rights">
                            {rights.map((line) => (
                                <li key={line}>{line}</li>
                            ))}
                        </ul>
                        <p class="t-caption m-0">{t.rightsNote}</p>
                    </WinSection>
                )}
            </Sheet>
        );

    if (hidden) return null;
    const word = own ?? words[0] ?? null;
    const later = steps.some((step) => step.state === 'later');
    // «Скажите: «<слово>, как дела?»» — только персонажу и слову; приложению — его фраза, как им пользоваться или просто «Готово».
    const talks = character || isCallWord(entry);
    const open = !character && phase === 'end' ? openTarget({ ...entry, installed: entry.version }, panel.go) : null;
    return (
        <Sheet
            title={phase === 'end' ? t.doneTitle : t.busyTitle(t.quote(entry.title))}
            about={phase === 'end' ? undefined : t.later}
            onClose={close}
            footer={
                <>
                    {failed && (
                        <button class="btn btn-plain" onClick={retry}>
                            {t.retry}
                        </button>
                    )}
                    {open ? (
                        <button
                            class="btn btn-main"
                            onClick={() => {
                                close();
                                open();
                            }}
                        >
                            {t.open}
                        </button>
                    ) : (
                        <button class={`btn ${phase === 'end' ? 'btn-main' : 'btn-quiet'}`} onClick={phase === 'end' ? close : () => setHidden(true)}>
                            {phase === 'end' ? t.close : t.hide}
                        </button>
                    )}
                </>
            }
        >
            <div class="win-group" role="list">
                {steps.map((step) => (
                    <div key={step.key} role="listitem">
                        <WinRow icon={<StepMark state={step.state} />} title={step.title} sub={step.note} />
                    </div>
                ))}
            </div>
            {phase === 'end' && (
                <p class="wz-say" role="status">
                    {later
                        ? t.readyLater
                        : talks
                          ? word
                              ? t.sayWord(word)
                              : character
                                ? t.sayAlice(name)
                                : t.readyApp(t.quote(entry.title))
                          : (appHint ?? t.readyApp(t.quote(entry.title)))}
                </p>
            )}
        </Sheet>
    );
}

function StepMark({ state }: { state: StepState }) {
    if (state === 'run') return <span class="spinner spinner-sm" aria-hidden="true" />;
    if (state === 'done')
        return (
            <span class="wz-mark is-done" aria-hidden="true">
                <IconCheck size={14} />
            </span>
        );
    return <span class={`wz-mark is-${state}`} aria-hidden="true" />;
}

/**
 * Пилюля хода установки рядом с доком (как «Ждёт вас»): мастер свёрнут — «Ставлю «Джарвис» · 3 из 5», готово —
 * «Готово: «Джарвис»»; нажатие разворачивает мастер. Видна с любого экрана (проверка начисто 18.11, п. 26).
 */
export function InstallPill({ place }: { place: 'top' | 'side' | 'onb' }) {
    const [, rerender] = useState(0);
    useEffect(() => onInstallProgress(() => rerender((n) => n + 1)), []);
    const progress = installProgress();
    const waiting = usePendingInstall();
    const settling = useSettling();
    const voiceAsks = useVoiceAttention();
    const t = tr.wizard;
    // Голос ждёт ответа владельца (проверка начисто 19.11): вопрос важнее хода — «… ждёт вашего решения».
    if (voiceAsks && (!progress || progress.hidden)) return <VoiceWaitingPill place={place} />;
    // Персонаж стоит, голос ещё ставится (п. 67): мастер кончил или закрыт — пилюля говорит ход, нажатие — окно хода.
    if (settling && (!progress || (progress.hidden && progress.finished)))
        return (
            <button type="button" class={`waiting-pill waiting-pill-${place}`} onClick={showSettling}>
                {settlingLine(settling)}
            </button>
        );
    // Выбран, пока колонка не центр (п. 63): «Джарвис» — ждёт колонку; центр готов — «готовлю у колонки».
    if (!progress && waiting)
        return (
            <span class={`waiting-pill waiting-pill-${place}${waiting.problem ? ' is-problem' : ''}`} role="status" title={waiting.problem}>
                {waiting.step ? t.pillPreparing(t.quote(waiting.title)) : t.pillWaits(t.quote(waiting.title))}
            </span>
        );
    if (!progress?.hidden) return null;
    return (
        <button type="button" class={`waiting-pill waiting-pill-${place}`} onClick={showInstallWizard}>
            {progress.finished
                ? t.pillDone(t.quote(progress.title))
                : progress.detail
                  ? t.pillDetail(t.quote(progress.title), progress.detail)
                  : t.pill(t.quote(progress.title), progress.done, progress.total)}
        </button>
    );
}
