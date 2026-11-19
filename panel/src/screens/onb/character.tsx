// Первые шаги → «Персонаж» (холст «Онбординг»: Character, CharacterStation, PhoneCharacter). Карточки поставленных
// персонажей с цветным «лицом», фразой и «Послушать»; нажатие выбирает персонажа сразу (POST …/choose), как «Выбрать»
// в «Персонажах» (screens/characters.tsx): голосу без модели — окно «нужна модель», памяти впритык — второе нажатие
// «всё равно». Шапку и кнопки внизу рисует рамка.
// DuoWave уже на колонке (пульт стал помощником): характер — настройка колонки, её мозг говорит им; голос — этого
// компьютера, колонка говорит его голосом (gateway/src/characters/mod.rs, station_voice). Выбор идёт в оба места: здесь
// — голос и его модель, на колонке — характер.

import { useEffect, useRef, useState } from 'preact/hooks';
import type { InstallProgress } from '../../magicplace/wizard-state';

import { CharacterView, CharactersView, api, can, isTight, needsOf, problemOf, stationCall } from '../../api';
import { VoiceWizard } from '../voice-lab';
import { stopAudio, useAudio } from '../../audio';
import { tr } from '../../i18n';
import { usePanel } from '../../shell/context';
import { VoiceMark, markOf, useMediaUrl } from '../../ui/media';
import { useNeedsFlow } from '../../ui/needs';
import { installProgress, onInstallProgress, showInstallWizard } from '../../magicplace/wizard-state';
import { Invite } from './invite';
import { linkActive, linkNow, moveNow, useSpeakerLink } from './speaker';
import { useOnboarding } from './nav';
import '../../ui/onb-character.css';

/** Ключ звука этого шага: уходя со шага, его звук останавливаем. */
const SOUND = 'onb-character:';
/** Сколько «звучит» карточка без фразы-пробника — как на холсте. */
const FAKE_MS = 3200;

type Face = 'face-divi' | 'face-violet' | 'face-warm' | 'face-cool';
const CYCLE: Face[] = ['face-cool', 'face-violet', 'face-warm'];

/** Оттенок (0–360) и насыщенность (0–1) цвета #RRGGBB; не цвет — null. */
function hueOf(color?: string | null): { hue: number; sat: number } | null {
    const match = color && /^#([0-9a-f]{6})$/i.exec(color);
    if (!match) return null;
    const n = parseInt(match[1], 16);
    const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => v / 255);
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    const d = max - min;
    if (d === 0) return { hue: 0, sat: 0 };
    const light = (max + min) / 2;
    const sat = d / (1 - Math.abs(2 * light - 1));
    const hue = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    return { hue: hue * 60, sat };
}

/** ДиВи — помощник DuoWave: только у него лаймовое лицо с глазами. */
const isDivi = (character: CharacterView): boolean => character.builtin && (character.id === 'divi' || /^(дива?и|deevi|divi)$/i.test(character.title.trim()));

/** Лицо карточки: ДиВи — фирменное; цвет персонажа, если он близок к одному из лиц; иначе — по кругу. */
function faceOf(character: CharacterView, at: number): Face {
    if (isDivi(character)) return 'face-divi';
    // Встроенная Алиса — её знак, волна на фиолетовом (как её аватар везде в пульте, п. 44).
    if (markOf(character) === 'voice') return 'face-violet';
    const tone = hueOf(character.color);
    if (tone) {
        if (tone.sat < 0.18) return 'face-cool';
        if (tone.hue >= 250 && tone.hue < 320) return 'face-violet';
        if (tone.hue >= 320 || tone.hue < 55) return 'face-warm';
        if (tone.hue >= 170 && tone.hue < 250) return 'face-cool';
    }
    return CYCLE[at % CYCLE.length];
}

/** Своя картинка персонажа (аватар пакета) — фоном лица; подпись остаётся поверх. */
function FaceImage({ src }: { src?: string | null }) {
    const url = useMediaUrl(src);
    return url ? <img class="onb-ch-img" src={url} alt="" /> : null;
}

/** Порядок карточек: встроенный первым, потом остальные как прислал шлюз; не разобравшиеся — только если он выбран. */
function shown(view: CharactersView): CharacterView[] {
    const active = view.active?.id;
    const list = view.characters.filter((character) => !character.problem || character.id === active);
    return [...list.filter((character) => character.builtin), ...list.filter((character) => !character.builtin)];
}

type Note = { text: string; tone?: 'warn' | 'bad' } | null;

/**
 * Перенос DuoWave на колонку ещё идёт (шаг «Колонка» первых шагов его начал, п. 44): центр поднимается, его каталога
 * и персонажей ещё нет — шаг ждёт, а не показывает карточки, собранные не там. Смотрит, пока перенос идёт; закончился —
 * false, и шаг перечитывает всё.
 */
function useMoving(kick = 0): boolean {
    const [moving, setMoving] = useState(false);
    useEffect(() => {
        let alive = true;
        let timer: ReturnType<typeof setTimeout> | null = null;
        const look = async (): Promise<void> => {
            const view = await api.placement().catch(() => null);
            if (!alive) return;
            const running = view?.job?.status === 'running';
            setMoving(running);
            if (running) timer = setTimeout(() => void look(), 1500);
        };
        void look();
        return () => {
            alive = false;
            if (timer) clearTimeout(timer);
        };
    }, [kick]);
    return moving;
}

/** Свёрнутая установка персонажа (мастер п. 40): её ход — тот же, что на пилюле у дока (п. 45). */
function useInstalling(): InstallProgress | null {
    const [now, setNow] = useState<InstallProgress | null>(installProgress);
    useEffect(() => onInstallProgress(() => setNow(installProgress())), []);
    return now;
}

/** Карточка установки, пока мастер свёрнут: имя, ход («модель 37 %» или «2 из 5»); нажатие разворачивает мастер. */
function InstallingCard({ progress }: { progress: InstallProgress }) {
    const t = tr.onbCharacter;
    const name = progress.title.replace(/\s*\([^)]*\)\s*$/, '').trim() || progress.title;
    const how = progress.detail ?? t.installingSteps(progress.done, progress.total);
    return (
        <div class="pcard is-busy onb-ch-installing">
            <button type="button" class="pcard-pick" aria-label={t.installingOpen(name)} onClick={showInstallWizard}>
                <span class={`face face-cool${progress.image ? ' onb-ch-has-img' : ''}`}>
                    <FaceImage src={progress.image} />
                    <span class="face-line">{t.installingFace}</span>
                </span>
                <span class="pcard-name">
                    {name}
                    <span class="mark mark-busy onb-inv-spin" aria-hidden="true" />
                </span>
                <span class="pcard-about">{t.installing(how)}</span>
            </button>
        </div>
    );
}

export function CharacterStep() {
    const { wake, choice, goTo } = useOnboarding();
    const panel = usePanel();
    /** «Перенести сейчас» нажали: смотреть перенос заново. */
    const [kick, setKick] = useState(0);
    const transferring = useMoving(kick);
    // Путь с колонкой выбран, а центром она ещё не стала (не подключена, перенос не начался): персонажи и приглашение
    // легли бы на этот компьютер, а не к центру — шаг ждёт и здесь (п. 58).
    const centreReady = panel.role.role === 'helper' || panel.health?.placement?.role === 'station';
    const awaitingCentre = choice !== 'computer' && !centreReady;
    // Перенос идёт — ждём его (центр поднимается). Колонка выбрана, а переноса нет — шаг не пустой (п. 61): Алиса и
    // персонажи этого компьютера видны, приглашение тут же (своё и персонажи переедут с переносом, c28c4266), а сверху —
    // честно, что колонка ещё не центр, и «Перенести сейчас».
    // Ход подключения, начатый на шаге «Колонка» (п. 62): пульт ведёт его и здесь (после перезагрузки пульта его
    // продолжают первые шаги, resumeLink в onboarding.tsx).
    const live = useSpeakerLink();
    const linking = linkActive(live);
    const moving = transferring || linking;
    // Ход стоит: колонка не пришла, подключение не вышло, перенос прервался — дорога к шагу «Колонка», там видно что.
    const stuck = awaitingCentre && !moving && (live?.phase === 'timeout' || live?.phase === 'failed' || !!live?.moveError);
    const [moveNote, setMoveNote] = useState<string | null>(null);
    const [moveBusy, setMoveBusy] = useState(false);
    const startNow = async (): Promise<void> => {
        setMoveBusy(true);
        setMoveNote(null);
        try {
            const started = await moveNow(choice === 'station' ? 'station' : 'stationServer');
            if (!started) setMoveNote(tr.onbCharacter.moveNoStation);
            else setKick((n) => n + 1);
        } catch (e) {
            setMoveNote(problemOf(e));
        } finally {
            setMoveBusy(false);
        }
    };
    const installing = useInstalling();
    const stationKey = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const audio = useAudio();
    const [view, setView] = useState<CharactersView | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [selected, setSelected] = useState<string | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<Note>(null);
    /** Памяти впритык (428) у этого персонажа: следующее нажатие — «всё равно». */
    const [tight, setTight] = useState<string | null>(null);
    /** Карточка без фразы-пробника «звучит» на холсте: показываем то же ~3 с. */
    const [fake, setFake] = useState<string | null>(null);
    const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const playing = useRef<string | null>(null);
    playing.current = audio.playing;
    const mayEdit = can('persona.edit');
    const mayVoice = can('models.manage');
    /** Что колонка не приняла, пока ставилась модель голоса: окно модели закончит — скажем вместе. */
    const missed = useRef<string | null>(null);
    /** Мастер «Свой голос» открыт — с предложения под карточками, когда выбрана стандартная Алиса. */
    const [ownWizard, setOwnWizard] = useState(false);

    /**
     * Мастер «Свой голос» закрыт: голос снят и включён на этом компьютере — колонка переходит на голос компьютера, иначе
     * она так и отвечала бы Алисой (её выбор «чей голос» остаётся за ней). Не включён — ничего не трогаем.
     */
    const ownVoiceDone = async (): Promise<void> => {
        setOwnWizard(false);
        try {
            const active = (await api.models()).ttsActive.modelId;
            if (!active.startsWith('own:') || active.startsWith('own:c-')) return;
            if (stationKey) await stationCall(stationKey, 'POST', '/api/v1/models/home-voice', { voice: 'computer' });
            setNote({ text: tr.voiceHome.onbOwnOn });
        } catch (e) {
            setNote({ text: tr.voiceHome.onbOwnStationMissed(problemOf(e)), tone: 'warn' });
        }
    };

    /** Характер — и колонке, когда DuoWave на ней; не вышло — строка, что сказать. */
    const toStation = async (character: CharacterView): Promise<string | null> => {
        if (!stationKey) return null;
        try {
            await stationCall(stationKey, 'POST', `/api/v1/characters/${encodeURIComponent(character.id)}/choose`, {});
            return null;
        } catch (e) {
            return tr.onbCharacter.stationMissed(problemOf(e));
        }
    };

    const read = async (): Promise<CharactersView | null> => {
        try {
            const next = await api.characters();
            setView(next);
            setError(null);
            return next;
        } catch (e) {
            setError(problemOf(e));
            return null;
        }
    };
    const needs = useNeedsFlow((message) => {
        const left = missed.current;
        missed.current = null;
        setNote(left ? { text: `${message} ${left}`, tone: 'warn' } : { text: message });
        void read();
    });

    useEffect(() => {
        void read().then((first) => first && setSelected(first.active?.id ?? null));
        return () => {
            if (timer.current) clearTimeout(timer.current);
            if (playing.current?.startsWith(SOUND)) stopAudio();
        };
    }, []);

    // Перенос закончился — персонажи и выбор заново (п. 44); свёрнутая установка сделала шаг — тоже: она могла выбрать
    // персонажа, и шаг отмечает выбранного, а не прежнего (п. 45).
    const settled = useRef(moving);
    useEffect(() => {
        if (settled.current && !moving) void read().then((next) => next?.active?.id && setSelected(next.active.id));
        settled.current = moving;
    }, [moving]);
    const stepsDone = installing ? `${installing.done}/${installing.finished}` : '';
    useEffect(() => {
        if (installing?.kind !== 'character' || installing.done === 0) return;
        void read().then((next) => next?.active?.id && setSelected(next.active.id));
    }, [stepsDone]);

    const askModel = (character: CharacterView, wanted: NonNullable<CharacterView['voice']['needs']>): void =>
        needs.open({
            who: character.title,
            homeVoice: { character: character.id },
            needs: wanted,
            fallback: character.voice.fallback?.title,
            retry: async ({ install, anyway }) => {
                const result = await api.chooseCharacter(character.id, { install, anyway });
                missed.current = await toStation(character);
                return result;
            },
        });

    const pick = async (character: CharacterView): Promise<void> => {
        if (!mayEdit || busy || character.problem) return;
        const before = selected;
        const anyway = tight === character.id;
        setSelected(character.id);
        setBusy(character.id);
        setNote({ text: tr.onbCharacter.choosing(character.title) });
        try {
            const result = await api.chooseCharacter(character.id, anyway ? { anyway: true } : {});
            setTight(null);
            const left = await toStation(character);
            const state = result.voice?.state;
            const wanted = result.voice?.needs ?? character.voice.needs;
            if (state === 'needs' && wanted && !mayVoice) setNote({ text: tr.characters.spokenReserveRestricted });
            else if (state === 'needs' && wanted) {
                setNote({ text: tr.characters.spokenReserveNeeds });
                askModel(character, wanted);
            } else setNote({ text: result.voice?.pending ? tr.characters.chosenModelInstalling : tr.onbCharacter.chosen(character.title) });
            if (left) setNote({ text: left, tone: 'warn' });
            void read();
        } catch (e) {
            const wanted = needsOf(e);
            if (isTight(e)) {
                setTight(character.id);
                setSelected(before);
                setNote({ text: `${problemOf(e)} ${tr.onbCharacter.pickAnyway}`, tone: 'warn' });
            } else if (wanted && mayVoice) {
                setNote(null);
                askModel(character, wanted);
            } else {
                setSelected(before);
                setNote({ text: problemOf(e), tone: 'bad' });
            }
        } finally {
            setBusy(null);
        }
    };

    const listen = (character: CharacterView): void => {
        const url = character.samples[0]?.url;
        if (timer.current) clearTimeout(timer.current);
        if (url) {
            setFake(null);
            void audio.toggle(SOUND + character.id, () => api.media(url));
            return;
        }
        // Фразы нет: звук другой карточки останавливаем и «звучим» как холст — или гасим, если нажали второй раз.
        if (playing.current?.startsWith(SOUND)) stopAudio();
        // Алиса — голос самой колонки, записи её голоса у нас нет и быть не может: честно, как её послушать (п. 44).
        if (markOf(character) === 'voice') return setNote({ text: tr.onbCharacter.aliceListen });
        if (fake === character.id) return setFake(null);
        setFake(character.id);
        timer.current = setTimeout(() => setFake(null), FAKE_MS);
    };

    const list = view ? shown(view) : [];
    // Выбрана стандартная (встроенная) — Алиса или ДиВи: рядом — «или ваш голос» (проверка начисто 19.11).
    const standard = list.find((character) => character.id === selected)?.builtin ?? false;
    /** Свёрнутая установка персонажа: своей карточкой, пока его нет в списке, а встал — ходом на его карточке. */
    const going = installing && installing.kind === 'character' && installing.hidden && !installing.finished ? installing : null;
    let others = 0;
    const footNote: Note = note ?? (mayEdit ? null : { text: tr.onbCharacter.onlyEditors });

    return (
        <div class="onb-ch">
            <div class="onb-ch-head">
                <h1 class="h2">
                    {tr.onbCharacter.title1}
                    <br />
                    {tr.onbCharacter.title2}
                </h1>
                <p class="lead">{tr.onbCharacter.lead}</p>
            </div>

            {/* Демо по приглашению (п. 42, 58) — отдельным блоком над карточками; перестанем раздавать ссылку — убрать эту
                строку и invite.tsx. */}
            {/* Всегда, и пока колонка настраивается (п. 63): ленту читает этот компьютер, выбор ждёт центр. И когда колонка
                уже центр — тоже этот компьютер: персонажи сразу, колонка догонит в фоне (замечание 3 теста 19.11). */}
            <Invite
                key={stationKey ?? 'own'}
                centre={stationKey}
                defer={awaitingCentre}
                centreProblem={stuck}
                onInstalled={() => void read().then((next) => next?.active?.id && setSelected(next.active.id))}
            />

            {awaitingCentre && !moving && (
                <div class="onb-ch-notcentre" role="status">
                    <span class="small">
                        {moveNote ?? (stuck ? (live?.moveError || tr.onbCharacter.stationStuck) : tr.onbCharacter.stationNotCentre)}
                    </span>
                    {goTo && (stuck || moveNote === tr.onbCharacter.moveNoStation) && (
                        <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={() => goTo('speaker')}>
                            {tr.onbCharacter.toSpeaker}
                        </button>
                    )}
                    <button type="button" class="dw-btn dw-btn-sm dw-btn-dark" disabled={moveBusy} aria-busy={moveBusy || undefined} onClick={() => void startNow()}>
                        {tr.onbCharacter.moveNow}
                    </button>
                </div>
            )}

            {/* Перенос идёт: ход строкой сверху, карточки на месте (п. 61: Алиса видна всегда). */}
            {moving && (
                <div class="onb-ch-notcentre is-moving" role="status">
                    <span class="small">
                        <span class="mark mark-busy onb-inv-spin" aria-hidden="true" />
                        {linkNow(live) ? tr.onbCharacter.stationNow(linkNow(live) as string) : tr.onbCharacter.stationSettingUp}
                    </span>
                    {goTo && live && (
                        <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={() => goTo('speaker')}>
                            {tr.onbCharacter.toSpeaker}
                        </button>
                    )}
                </div>
            )}

            {!view ? (
                <p class="small onb-ch-p" role={error ? 'alert' : 'status'}>
                    {error ?? tr.onbCharacter.reading}
                </p>
            ) : list.length === 0 ? (
                <p class="small onb-ch-p">{tr.onbCharacter.none}</p>
            ) : (
                <div class="onb-ch-grid">
                    {going && !list.some((character) => character.package?.id === going.id) && <InstallingCard progress={going} />}
                    {list.map((character) => {
                        const face = faceOf(character, isDivi(character) ? 0 : others++);
                        const on = selected === character.id;
                        const sample = character.samples[0];
                        const sounding = fake === character.id || audio.playing === SOUND + character.id;
                        return (
                            <div key={character.id} class={`pcard${on ? ' on' : ''}${busy === character.id ? ' is-busy' : ''}`}>
                                <button
                                    type="button"
                                    class="pcard-pick"
                                    aria-pressed={on ? 'true' : 'false'}
                                    disabled={!mayEdit || !!character.problem}
                                    onClick={() => void pick(character)}
                                >
                                    <span class={`face ${face}${character.avatar ? ' onb-ch-has-img' : markOf(character) === 'voice' ? ' onb-ch-has-mark' : ''}`}>
                                        <FaceImage src={character.avatar} />
                                        <span class="face-line">
                                            {sample?.text ? tr.onbCharacter.quoted(sample.text) : character.description}
                                        </span>
                                        {!character.avatar && markOf(character) === 'voice' && (
                                            <span class="onb-ch-mark" aria-hidden="true">
                                                <VoiceMark />
                                            </span>
                                        )}
                                        {face === 'face-divi' && (
                                            <span class="divi blink onb-ch-divi">
                                                <svg viewBox="0 0 256 256" aria-hidden="true">
                                                    <path class="divi-eye divi-eyes" d="M98 110V138M158 110V138" />
                                                </svg>
                                            </span>
                                        )}
                                    </span>
                                    <span class="pcard-name">
                                        {character.title}
                                        <span class="radio" />
                                    </span>
                                    <span class="pcard-about">
                                        {going && character.package?.id === going.id
                                            ? tr.onbCharacter.installing(going.detail ?? tr.onbCharacter.installingSteps(going.done, going.total))
                                            : character.description}
                                    </span>
                                </button>
                                {going && character.package?.id === going.id && (
                                    <button type="button" class="listen" onClick={showInstallWizard}>
                                        {tr.onbCharacter.installingShow}
                                    </button>
                                )}
                                <button
                                    type="button"
                                    class="listen"
                                    aria-pressed={sounding}
                                    aria-label={sounding ? tr.onbCharacter.stop : tr.onbCharacter.listenTo(character.title)}
                                    onClick={() => listen(character)}
                                >
                                    {sounding ? (
                                        <>
                                            <span class="wave" aria-hidden="true">
                                                <i />
                                                <i />
                                                <i />
                                                <i />
                                            </span>
                                            {tr.onbCharacter.playing}
                                        </>
                                    ) : (
                                        <>
                                            <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                                <path d="M8 5v14l11-7z" />
                                            </svg>
                                            {tr.onbCharacter.listen}
                                        </>
                                    )}
                                </button>
                            </div>
                        );
                    })}
                </div>
            )}

            {standard && mayVoice && !awaitingCentre && (
                <div class="onb-ch-own">
                    <span class="onb-ch-own-text">
                        <b>{tr.voiceHome.onbOwnTitle}</b>
                        <span class="small">{tr.voiceHome.onbOwnAbout}</span>
                    </span>
                    <button type="button" class="dw-btn dw-btn-sm dw-btn-dark" onClick={() => setOwnWizard(true)}>
                        {tr.voiceHome.onbOwnCreate}
                    </button>
                </div>
            )}

            <div class="onb-ch-foot">
                <span class="onb-ch-more">
                    <span class="mp">mp</span>
                    <span class="small">{tr.onbCharacter.more}</span>
                </span>
                {footNote ? (
                    <span
                        class={`small${footNote.tone ? ` onb-ch-${footNote.tone}` : ''}`}
                        role={footNote.tone === 'bad' ? 'alert' : 'status'}
                    >
                        {footNote.text}
                    </span>
                ) : (
                    <span class="small">{wake === 'station' ? tr.onbCharacter.noteStation : tr.onbCharacter.noteComputer}</span>
                )}
            </div>
            {needs.dialog}
            {ownWizard && <VoiceWizard onClose={() => void ownVoiceDone()} />}
        </div>
    );
}
