// «Как говорить с DuoWave» (docs/plans/mic-mode.md, «Выбор»): через колонку с Алисой или микрофоном этого компьютера.
// Выбор — один, и он шлюза на компьютере: первый запуск спрашивает его после пароля, пустой стол предлагает рядом с
// «Подключите колонку», «Настройки» держат карточкой сверху страницы. Выбран микрофон — список готовности: слух
// (распознавание на компьютере), голос (свой голос выбран, его сервер готов), мозг (модель выбрана); чего нет — кнопка
// на нужный экран. Переход на колонку спрашивает «Вернуть Алису и колонку?», на микрофон — говорит, что уйдёт.

import { useEffect, useRef, useState } from 'preact/hooks';

import { KeyActivation, ModelsView, TalkVoiceChange, TalkVoiceView, VoiceInput, api, can, problemOf } from '../api';
import { IconCheck, IconLaptop, IconMic, IconSpeaker } from '../icons';
import { Recorder, recordingSupport } from '../recorder';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { noteVoiceInput, useOnComputer } from '../voice-input';
import { startRecorder } from '../voice/mic';
import { AudioDevice, audioDevices, chosenInput, chosenOutput, outputChoosable, setChosenInput, setChosenOutput } from '../voice/devices';
import { micSounds, playTone, rerouteTones, setMicSounds } from '../voice/tones';
import { DEFAULT_TALK_KEY, TalkKey, keyLabel, keyOf, same, setChoosingTalkKey, setTalkKey, useTalkKeyChoice } from '../voice/talk-key';
import { keyName, readAppVoice, useAppVoice } from '../voice/app-voice';
import { tr } from '../i18n';
import { languageName, localAsrNeedsCard, useLocalAsr } from './local-asr';

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
 * запускается не сразу. `win` — строками окна (ui/window.css), а не карточки страницы.
 */
export function Readiness({ onGo, win = false }: { onGo?: () => void; win?: boolean }) {
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
    // Язык дома, которого не знает ни одна модель здесь: слух работает, но этот язык не поймёт.
    const neededFor = local?.language && local.needs ? local.language : null;
    const needed = local?.needs && neededFor ? (models?.asr.find((card) => card.model === local.needs)?.title ?? local.needs) : null;
    const brain = health?.brain;
    const thinks = !!brain && brain.provider !== 'none';
    const ownVoice = models?.voice === 'own' && !!models.ttsActive.modelId;
    const items: Ready[] = [
        {
            key: 'hear',
            title: tr.voiceInput.hearTitle,
            ok: hears && !preparing && !needed,
            state: preparing
                ? tr.voiceInput.hearPreparing
                : !hears
                  ? tr.voiceInput.hearOff
                  : needed && neededFor
                    ? tr.voiceInput.hearNeeds(languageName(neededFor), needed)
                    : tr.voiceInput.hearOk,
            fix: preparing ? undefined : !hears ? { label: tr.voiceInput.hearFix, go: go('settings', 'station') } : needed ? { label: tr.voiceInput.hearNeedsFix, go: go('settings', 'station') } : undefined,
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
    if (win)
        return (
            <>
                <ul class="win-group m-0 p-0" aria-label={tr.voiceInput.readinessLabel}>
                    {items.map((item) => (
                        <li key={item.key} class="win-row">
                            <span class={`ready-mark${item.ok ? ' is-ok' : ''}`} aria-hidden="true">
                                {item.ok && <IconCheck size={12} />}
                            </span>
                            <span class="win-row-main">
                                <span class="win-row-t">{item.title}</span>
                                <span class="win-row-s">{item.state}</span>
                            </span>
                            {item.fix && (
                                <button class="btn btn-plain btn-sm" onClick={item.fix.go}>
                                    {item.fix.label}
                                </button>
                            )}
                        </li>
                    ))}
                </ul>
                <p class="win-note">{ready ? tr.voiceInput.allReady : tr.voiceInput.whenReady}</p>
            </>
        );
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

/** Проверка микрофона разрешила его: выбор устройств перечитывает их названия. */
const MIC_ALLOWED = 'duowave:mic-allowed';

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
            // Тот микрофон, что выбран ниже: проверяют его.
            recorder.current = await startRecorder((db) => {
                const value = Math.max(0, Math.min(1, (db + 55) / 45));
                setLevel(value);
                if (value > 0.35) setHeard(true);
            });
            timer.current = window.setTimeout(stop, 5000);
            // Микрофон разрешён: теперь у устройств есть названия.
            window.dispatchEvent(new Event(MIC_ALLOWED));
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
            <AudioDeviceChoice />
            <MicSoundsChoice />
        </div>
    );
}

/**
 * Откуда слушать и куда говорить (voice/devices.ts): «Автоматически» — что даёт система, или выбранное устройство.
 * Названия устройств браузер показывает после первого разрешения микрофона; выход — там, где браузер даёт его выбрать.
 */
function AudioDeviceChoice() {
    const [devices, setDevices] = useState<{ inputs: AudioDevice[]; outputs: AudioDevice[] }>({ inputs: [], outputs: [] });
    const [input, setInput] = useState(chosenInput());
    const [output, setOutput] = useState(chosenOutput());
    useEffect(() => {
        if (!recordingSupport().ok) return;
        const load = () => void audioDevices().then(setDevices).catch(() => undefined);
        load();
        navigator.mediaDevices.addEventListener?.('devicechange', load);
        window.addEventListener(MIC_ALLOWED, load);
        return () => {
            navigator.mediaDevices.removeEventListener?.('devicechange', load);
            window.removeEventListener(MIC_ALLOWED, load);
        };
    }, []);
    const named = devices.inputs.some((device) => !device.label.startsWith('🎙'));
    return (
        <div class="flex flex-col gap-10">
            <label class="flex flex-col gap-4">
                <span class="w-strong">{tr.voiceInput.inputTitle}</span>
                <select
                    class="field"
                    value={input}
                    onChange={(e) => {
                        const id = (e.target as HTMLSelectElement).value;
                        setChosenInput(id);
                        setInput(id);
                    }}
                >
                    <option value="">{tr.voiceInput.deviceAuto}</option>
                    {devices.inputs.map((device) => (
                        <option key={device.id} value={device.id}>
                            {device.label}
                        </option>
                    ))}
                </select>
                {!named && devices.inputs.length > 0 && <span class="w-cap">{tr.voiceInput.namesAfterCheck}</span>}
            </label>
            {outputChoosable() && devices.outputs.length > 0 ? (
                <label class="flex flex-col gap-4">
                    <span class="w-strong">{tr.voiceInput.outputTitle}</span>
                    <select
                        class="field"
                        value={output}
                        onChange={(e) => {
                            const id = (e.target as HTMLSelectElement).value;
                            setChosenOutput(id);
                            setOutput(id);
                            rerouteTones();
                            playTone('heard');
                        }}
                    >
                        <option value="">{tr.voiceInput.deviceAuto}</option>
                        {devices.outputs.map((device) => (
                            <option key={device.id} value={device.id}>
                                {device.label}
                            </option>
                        ))}
                    </select>
                </label>
            ) : (
                <span class="w-cap">{tr.voiceInput.outputBySystem}</span>
            )}
        </div>
    );
}

/** Звуки микрофона (voice/tones.ts): «слушаю» и «услышала». Хранятся в этом браузере. */
function MicSoundsChoice() {
    const [on, setOn] = useState(micSounds());
    const change = (value: boolean) => {
        setMicSounds(value);
        setOn(value);
        if (value) playTone('listen');
    };
    return (
        <div class="w-row">
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="w-strong">{tr.voiceInput.micSoundsTitle}</span>
                <span class="w-cap">{tr.voiceInput.micSoundsText}</span>
            </span>
            <Switch on={on} label={tr.voiceInput.micSoundsTitle} onChange={change} />
        </div>
    );
}

/**
 * Клавиша «Сказать» (voice/talk-key.ts): какая сейчас и смена нажатием. Пока ждём нажатия, прежняя клавиша не слушает;
 * Esc — отмена.
 */
export function TalkKeyChoice() {
    const key = useTalkKeyChoice();
    const [waiting, setWaiting] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    // С ухом приложения клавиша — для всех программ: она хранится в шлюзе, приложение ставит её в системе (К4).
    const voice = useAppVoice();
    const everywhere = !!voice?.port && voice.here;
    const choose = (next: TalkKey) => {
        if (everywhere) {
            const name = keyName(next);
            if (!name) return setProblem(tr.voiceInput.keyNotGlobal);
            void api
                .setTalkVoice({ key: name })
                .then(() => readAppVoice())
                .catch((error) => setProblem(problemOf(error)));
        }
        setTalkKey(next);
        setProblem(null);
    };
    useEffect(() => {
        if (!waiting) return;
        setChoosingTalkKey(true);
        const onKey = (event: KeyboardEvent) => {
            event.preventDefault();
            event.stopPropagation();
            if (event.code === 'Escape') return setWaiting(false);
            const next = keyOf(event);
            if (!next) return;
            if ('problem' in next) return setProblem(next.problem);
            choose(next);
            setWaiting(false);
        };
        window.addEventListener('keydown', onKey, true);
        return () => {
            window.removeEventListener('keydown', onKey, true);
            setChoosingTalkKey(false);
        };
    }, [waiting]);
    const label = keyLabel(key);
    return (
        <div class="flex flex-col gap-10">
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{waiting ? tr.voiceInput.keyPress : label}</span>
                    <span class="w-cap">
                        {waiting ? tr.voiceInput.keyPressSub : everywhere ? tr.voiceInput.keyEverywhere(label) : tr.voiceInput.keyAbout(label)}
                    </span>
                </span>
                {!waiting && !same(key, DEFAULT_TALK_KEY) && (
                    <button class="w-btn" onClick={() => choose(DEFAULT_TALK_KEY)}>
                        {tr.voiceInput.keyDefault(keyLabel(DEFAULT_TALK_KEY))}
                    </button>
                )}
                <button
                    class="w-btn"
                    onClick={() => {
                        setProblem(null);
                        setWaiting(!waiting);
                    }}
                >
                    {waiting ? tr.voiceInput.keyCancel : tr.voiceInput.keyChange}
                </button>
            </div>
            {problem && (
                <p role="alert" class="w-cap m-0 c-dot-offline">
                    {problem}
                </p>
            )}
            {everywhere && voice?.keyTaken && (
                <p role="alert" class="w-cap m-0 c-dot-offline">
                    {tr.voiceInput.keyTaken}
                </p>
            )}
            {everywhere && voice && <KeyActivationChoice voice={voice} onProblem={setProblem} />}
        </div>
    );
}

const ACTIVATIONS: KeyActivation[] = ['hold_or_toggle', 'push_to_talk', 'toggle'];

/** How the key drives a thought: Handy's three modes, taken as they are (the owner, 27.11). */
function KeyActivationChoice({ voice, onProblem }: { voice: TalkVoiceView; onProblem: (problem: string) => void }) {
    const now = voice.activation ?? 'hold_or_toggle';
    const title = (mode: KeyActivation) =>
        mode === 'push_to_talk' ? tr.voiceInput.activationPushToTalk : mode === 'toggle' ? tr.voiceInput.activationToggle : tr.voiceInput.activationHoldOrToggle;
    const about =
        now === 'push_to_talk' ? tr.voiceInput.activationPushToTalkAbout : now === 'toggle' ? tr.voiceInput.activationToggleAbout : tr.voiceInput.activationHoldOrToggleAbout;
    const pick = (mode: KeyActivation) =>
        void api
            .setTalkVoice({ activation: mode })
            .then(() => readAppVoice())
            .catch((error) => onProblem(problemOf(error)));
    return (
        <div class="flex flex-col gap-6">
            <span class="w-strong">{tr.voiceInput.activationTitle}</span>
            <div role="radiogroup" aria-label={tr.voiceInput.activationTitle} class="look-seg glass">
                {ACTIVATIONS.map((mode) => (
                    <button key={mode} role="radio" aria-checked={now === mode} disabled={!can('station.mode')} onClick={() => pick(mode)}>
                        {title(mode)}
                    </button>
                ))}
            </div>
            <span class="w-cap">{about}</span>
            <ToggleRow
                title={tr.voiceInput.autoEndTitle}
                text={tr.voiceInput.autoEndText}
                on={voice.autoEnd ?? true}
                disabled={!can('station.mode')}
                onChange={(autoEnd) =>
                    void api
                        .setTalkVoice({ autoEnd })
                        .then(() => readAppVoice())
                        .catch((error) => onProblem(problemOf(error)))
                }
            />
        </div>
    );
}

/** Строка с выключателем: заголовок, пояснение и сам выключатель. */
function ToggleRow({ title, text, on, disabled = false, onChange }: { title: string; text: string; on: boolean; disabled?: boolean; onChange: (on: boolean) => void }) {
    return (
        <div class="w-row">
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="w-strong">{title}</span>
                <span class="w-cap">{text}</span>
            </span>
            <Switch on={on} label={title} disabled={disabled} onChange={onChange} />
        </div>
    );
}

/**
 * Голос компьютера (docs/plans/audio-interface-3.md, «К5»): приложение DuoWave на этом Mac слышит и говорит само —
 * при закрытом окне, по клавише из любой программы; «Готово» говорит сам компьютер. Здесь — готов ли он, звуки,
 * перебивание голосом после проверки эха, полоса поверх окон и слово.
 */
export function AppVoiceCard() {
    const voice = useAppVoice();
    const [checking, setChecking] = useState(false);
    const [checked, setChecked] = useState<string | null>(null);
    if (!voice?.port) return null;
    const admin = can('station.mode');
    const change = (patch: TalkVoiceChange) =>
        void api
            .setTalkVoice(patch)
            .then(() => readAppVoice())
            .catch(() => undefined);
    const status = voice.access === 'denied' ? tr.voiceInput.appNoAccess : voice.busy ? tr.voiceInput.appBusy : tr.voiceInput.appReady;
    const echoOk = !!voice.echo?.ok;
    const echoText = checked ?? (voice.echo ? (voice.echo.ok ? tr.voiceInput.echoOk : tr.voiceInput.echoHeard) : tr.voiceInput.echoText);
    const check = async () => {
        setChecking(true);
        setChecked(null);
        try {
            const result = await api.voiceCheck();
            setChecked(result.ok ? tr.voiceInput.echoOk : tr.voiceInput.echoHeard);
        } catch (error) {
            setChecked(problemOf(error));
        } finally {
            setChecking(false);
            void readAppVoice();
        }
    };
    return (
        <div class="flex flex-col gap-10">
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{tr.voiceInput.appTitle}</span>
                    <span class="w-cap">{status}</span>
                </span>
            </div>
            <ToggleRow title={tr.voiceInput.micSoundsTitle} text={tr.voiceInput.micSoundsText} on={voice.sounds} disabled={!admin} onChange={(sounds) => change({ sounds })} />
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{tr.voiceInput.echoTitle}</span>
                    <span class="w-cap">{echoText}</span>
                </span>
                <button class="w-btn" disabled={checking || !admin || !voice.here} onClick={() => void check()}>
                    {checking ? tr.voiceInput.echoChecking : tr.voiceInput.echoButton}
                </button>
            </div>
            <ToggleRow
                title={tr.voiceInput.bargeTitle}
                text={echoOk ? tr.voiceInput.bargeText : tr.voiceInput.bargeNeedsCheck}
                on={voice.bargeIn && echoOk}
                disabled={!admin || !echoOk}
                onChange={(bargeIn) => change({ bargeIn })}
            />
            <ToggleRow title={tr.voiceInput.stripTitle} text={tr.voiceInput.stripText} on={voice.strip} disabled={!admin} onChange={(strip) => change({ strip })} />
            {(voice.wordApp || voice.word) && (
                <ToggleRow title={tr.voiceInput.wordTitle} text={voice.word ? tr.voiceInput.wordOnText : tr.voiceInput.wordText} on={voice.word} disabled={!admin} onChange={(word) => change({ word })} />
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
                <Readiness onGo={onClose} win />
            </Sheet>
        );
    return (
        <Sheet
            title={tr.voiceInput.sheetTitle}
            about={tr.voiceInput.sheetAbout}
            onClose={onClose}
            footer={
                <button class="btn btn-plain" onClick={onClose}>
                    {tr.voiceInput.later}
                </button>
            }
        >
            <Options current={null} busy={busy} onPick={(input) => void pick(input)} />
            {problem && (
                <p role="alert" class="win-callout m-0">
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
