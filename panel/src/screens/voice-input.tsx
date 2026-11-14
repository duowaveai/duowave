// «Как говорить с DuoWave» (docs/plans/mic-mode.md, «Выбор»): через колонку с Алисой или микрофоном этого компьютера.
// Выбор — один, и он шлюза на компьютере: первый запуск спрашивает его после пароля, пустой стол предлагает рядом с
// «Подключите колонку», «Настройки» держат карточкой сверху страницы. Выбран микрофон — список готовности: слух
// (распознавание на компьютере), голос (свой голос выбран, его сервер готов), мозг (модель выбрана); чего нет — кнопка
// на нужный экран. Переход на колонку спрашивает «Вернуть Алису и колонку?», на микрофон — говорит, что уйдёт.

import { useEffect, useRef, useState } from 'preact/hooks';

import { ModelsView, VoiceInput, api, can, problemOf } from '../api';
import { IconCheck, IconLaptop, IconMic, IconSpeaker } from '../icons';
import { Recorder, recordingSupport } from '../recorder';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { Sheet } from '../ui/sheet';
import { noteVoiceInput, useOnComputer } from '../voice-input';
import { LISTEN_RATE } from '../voice/mic';
import { tr } from '../i18n';
import { localAsrNeedsCard, useLocalAsr } from './local-asr';

/** Выбор есть только у шлюза на компьютере: у шлюза на самой колонке и у её помощника — колонка. */
export function useTalkChoiceHere(): boolean {
    const panel = usePanel();
    return panel.health?.placement?.role !== 'station' && panel.role.role !== 'helper';
}

/** Записать выбор: шлюз переходит в «Своё» и запирает его или возвращает прежний режим; пульт перечитает сам. */
async function choose(input: VoiceInput): Promise<void> {
    const chosen = await api.setTalkInput(input);
    noteVoiceInput(chosen.input);
}

const toComputer = () => ({
    title: tr.voiceInput.toComputerTitle,
    text: tr.voiceInput.toComputerText,
    ok: tr.voiceInput.toComputerOk,
});
const toStation = () => ({
    title: tr.voiceInput.toStationTitle,
    text: tr.voiceInput.toStationText,
    ok: tr.voiceInput.toStationOk,
});

interface Option {
    input: VoiceInput;
    title: string;
    sub: string;
    icon: typeof IconMic;
}

const options = (): Option[] => [
    { input: 'computer', title: tr.voiceInput.optionComputerTitle, sub: tr.voiceInput.optionComputerSub, icon: IconLaptop },
    { input: 'station', title: tr.voiceInput.optionStationTitle, sub: tr.voiceInput.optionStationSub, icon: IconSpeaker },
];

/** Два варианта выбора; `current` отмечен. */
function Options({ current, busy, onPick }: { current: VoiceInput | null; busy: boolean; onPick: (input: VoiceInput) => void }) {
    return (
        <div class="first-run-options" role="radiogroup" aria-label={tr.voiceInput.howToTalk}>
            {options().map((option) => {
                const on = current === option.input;
                const { icon: OptionIcon } = option;
                return (
                    <button
                        key={option.input}
                        role="radio"
                        aria-checked={on}
                        class={`first-run-option talk-option${on ? ' is-on' : ''}`}
                        disabled={busy}
                        onClick={() => onPick(option.input)}
                    >
                        <span class="talk-option-icon" aria-hidden="true">
                            <OptionIcon size={20} />
                        </span>
                        <b>{option.title}</b>
                        <span class="w-cap">{option.sub}</span>
                        {on && (
                            <span class="talk-option-on">
                                <IconCheck size={12} /> {tr.voiceInput.chosen}
                            </span>
                        )}
                    </button>
                );
            })}
        </div>
    );
}

/** Карточка «Настроек»: выбор с вопросом перед переменой. Без права на режим — только что выбрано. */
export function TalkChoice() {
    const computer = useOnComputer();
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const may = can('station.mode');
    const pick = async (input: VoiceInput) => {
        if (input === (computer ? 'computer' : 'station')) return;
        if (!(await ask(input === 'computer' ? toComputer() : toStation()))) return;
        setBusy(true);
        setProblem(null);
        try {
            await choose(input);
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(false);
        }
    };
    return (
        <>
            <Options current={computer ? 'computer' : 'station'} busy={busy || !may} onPick={(input) => void pick(input)} />
            {!may && <p class="w-cap m-0">{tr.voiceInput.adminChooses}</p>}
            {problem && (
                <p role="alert" class="w-cap m-0 c-dot-offline">
                    {problem}
                </p>
            )}
        </>
    );
}

interface Ready {
    key: string;
    title: string;
    ok: boolean;
    /** Что сейчас: «распознаёт этот компьютер», «голос не выбран». */
    state: string;
    /** Куда идти, если не готово. */
    fix?: { label: string; go: () => void };
}

/**
 * Список готовности режима «Микрофон»: слух, голос, мозг. Перечитывает голос, пока открыт: сервер своего голоса
 * запускается не сразу.
 */
export function Readiness({ onGo }: { onGo?: () => void }) {
    const panel = usePanel();
    const [models, setModels] = useState<ModelsView | null>(null);
    const [local] = useLocalAsr();
    const version = panel.version;
    useEffect(() => {
        if (!can('station.view')) return;
        let alive = true;
        const read = () =>
            api
                .models()
                .then((view) => alive && setModels(view))
                .catch(() => undefined);
        void read();
        const timer = window.setInterval(() => void read(), 4000);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, [version]);
    const go = (section: string, tab: string) => () => {
        onGo?.();
        panel.go(section, tab);
    };
    const health = panel.health;
    const hears = !!health?.speech && health.speech.asr !== 'off';
    const preparing = localAsrNeedsCard(local);
    const brain = health?.brain;
    const thinks = !!brain && brain.provider !== 'none';
    const ownVoice = models?.voice === 'own' && !!models.ttsActive.modelId;
    const items: Ready[] = [
        {
            key: 'hear',
            title: tr.voiceInput.hearTitle,
            ok: hears && !preparing,
            state: preparing ? tr.voiceInput.hearPreparing : hears ? tr.voiceInput.hearOk : tr.voiceInput.hearOff,
            fix: hears || preparing ? undefined : { label: tr.voiceInput.hearFix, go: go('settings', 'station') },
        },
        {
            key: 'voice',
            title: tr.voiceInput.voiceTitle,
            ok: ownVoice && !!models?.ttsUp,
            state: !models ? tr.voiceInput.voiceChecking : !ownVoice ? tr.voiceInput.voiceNotChosen : models.ttsUp ? tr.voiceInput.voiceReady : tr.voiceInput.voiceStarting,
            fix: models && !ownVoice ? { label: tr.voiceInput.voiceFix, go: go('character', 'voice') } : undefined,
        },
        {
            key: 'brain',
            title: tr.voiceInput.brainTitle,
            ok: thinks,
            state: thinks ? (brain.profiles?.fast?.model ?? brain.models.fast) || tr.voiceInput.brainChosen : tr.voiceInput.brainNone,
            fix: thinks ? undefined : { label: tr.voiceInput.brainFix, go: go('character', 'brain') },
        },
    ];
    const ready = items.every((item) => item.ok);
    return (
        <div class="flex flex-col gap-10">
            <ul class="w-rows ready-list" aria-label={tr.voiceInput.readinessLabel}>
                {items.map((item) => (
                    <li key={item.key} class="w-row">
                        <span class={`ready-mark${item.ok ? ' is-ok' : ''}`} aria-hidden="true">
                            {item.ok && <IconCheck size={12} />}
                        </span>
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">{item.title}</span>
                            <span class="w-cap">{item.state}</span>
                        </span>
                        {item.fix && (
                            <button class="w-btn" onClick={item.fix.go}>
                                {item.fix.label}
                            </button>
                        )}
                    </li>
                ))}
            </ul>
            <p class="w-cap m-0">{ready ? tr.voiceInput.allReady : tr.voiceInput.whenReady}</p>
        </div>
    );
}

/** Проверка микрофона: пять секунд показывает уровень звука — слышит ли пульт. Ничего не пишет и никуда не шлёт. */
export function MicCheck() {
    const [level, setLevel] = useState<number | null>(null);
    const [heard, setHeard] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const recorder = useRef<Recorder | null>(null);
    const timer = useRef(0);
    const stop = () => {
        window.clearTimeout(timer.current);
        recorder.current?.cancel();
        recorder.current = null;
        setLevel(null);
    };
    useEffect(() => stop, []);
    const start = async () => {
        const support = recordingSupport();
        if (!support.ok) return setProblem(support.reason ?? tr.voiceInput.micUnavailable);
        setProblem(null);
        setHeard(false);
        setLevel(0);
        try {
            recorder.current = await Recorder.start(
                undefined,
                (db) => {
                    const value = Math.max(0, Math.min(1, (db + 55) / 45));
                    setLevel(value);
                    if (value > 0.35) setHeard(true);
                },
                { rate: LISTEN_RATE, processed: true },
            );
            timer.current = window.setTimeout(stop, 5000);
        } catch (error) {
            setLevel(null);
            setProblem(error instanceof Error ? error.message : String(error));
        }
    };
    const on = level !== null;
    return (
        <div class="flex flex-col gap-10">
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{tr.voiceInput.micCheckTitle}</span>
                    <span class="w-cap">
                        {on ? tr.voiceInput.micCheckOn : heard ? tr.voiceInput.micHeard : tr.voiceInput.micCheckIdle}
                    </span>
                </span>
                <button class="w-btn" onClick={() => (on ? stop() : void start())}>
                    {on ? tr.voiceInput.stop : tr.voiceInput.check}
                </button>
            </div>
            <div class="mic-check" role="meter" aria-label={tr.voiceInput.micLevel} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round((level ?? 0) * 100)}>
                <span style={{ width: `${Math.round((level ?? 0) * 100)}%` }} />
            </div>
            {problem && (
                <p role="alert" class="w-cap m-0 c-dot-offline">
                    {problem}
                </p>
            )}
        </div>
    );
}

/**
 * Первый запуск на компьютере: после пароля — «Как будете говорить с DuoWave?». Колонка — дальше её подключение
 * (`onStation`); микрофон — выбор записывается, и лист показывает готовность.
 */
export function TalkChoiceSheet({ onClose, onStation }: { onClose: () => void; onStation: () => void }) {
    const computer = useOnComputer();
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const pick = async (input: VoiceInput) => {
        if (input === 'station') return onStation();
        setBusy(true);
        setProblem(null);
        try {
            await choose('computer');
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(false);
        }
    };
    if (computer)
        return (
            <Sheet title={tr.voiceInput.sheetComputerTitle} about={tr.voiceInput.sheetComputerAbout} onClose={onClose}>
                <Readiness onGo={onClose} />
            </Sheet>
        );
    return (
        <Sheet
            title={tr.voiceInput.sheetTitle}
            about={tr.voiceInput.sheetAbout}
            onClose={onClose}
            footer={
                <button class="dash-done" onClick={onClose}>
                    {tr.voiceInput.later}
                </button>
            }
        >
            <Options current={null} busy={busy} onPick={(input) => void pick(input)} />
            {problem && (
                <p role="alert" class="w-cap m-0 c-dot-offline">
                    {problem}
                </p>
            )}
        </Sheet>
    );
}

/** Пустой стол, рядом с «Подключите колонку»: «Или говорите микрофоном этого компьютера». */
export function MicFirstCard() {
    const here = useTalkChoiceHere();
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    if (!here || !can('station.mode')) return null;
    const start = async () => {
        if (!(await ask(toComputer()))) return;
        setBusy(true);
        setProblem(null);
        try {
            await choose('computer');
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(false);
        }
    };
    return (
        <section class="card glass flex flex-col gap-12" aria-labelledby="mic-first-h">
            <h2 id="mic-first-h" class="t-title m-0">
                {tr.voiceInput.micFirstTitle}
            </h2>
            <p class="t-meta m-0 c-text">
                {tr.voiceInput.micFirstText}
            </p>
            <div>
                <button class="w-btn w-btn-ink" disabled={busy} onClick={() => void start()}>
                    <IconMic size={16} /> {busy ? tr.voiceInput.enabling : tr.voiceInput.talkWithMic}
                </button>
            </div>
            {problem && (
                <p role="alert" class="w-cap m-0 c-dot-offline">
                    {problem}
                </p>
            )}
        </section>
    );
}
