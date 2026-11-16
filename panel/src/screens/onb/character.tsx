// Первые шаги → «Персонаж» (холст «Онбординг»: Character, CharacterStation, PhoneCharacter). Карточки поставленных
// персонажей с цветным «лицом», фразой и «Послушать»; нажатие выбирает персонажа сразу (POST …/choose), как «Выбрать»
// в «Персонажах» (screens/characters.tsx): голосу без модели — окно «нужна модель», памяти впритык — второе нажатие
// «всё равно». Шапку и кнопки внизу рисует рамка.

import { useEffect, useRef, useState } from 'preact/hooks';

import { CharacterView, CharactersView, api, can, isTight, needsOf, problemOf } from '../../api';
import { stopAudio, useAudio } from '../../audio';
import { tr } from '../../i18n';
import { useMediaUrl } from '../../ui/media';
import { useNeedsFlow } from '../../ui/needs';
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

export function CharacterStep() {
    const { wake } = useOnboarding();
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
        setNote({ text: message });
        void read();
    });

    useEffect(() => {
        void read().then((first) => first && setSelected(first.active?.id ?? null));
        return () => {
            if (timer.current) clearTimeout(timer.current);
            if (playing.current?.startsWith(SOUND)) stopAudio();
        };
    }, []);

    const askModel = (character: CharacterView, wanted: NonNullable<CharacterView['voice']['needs']>): void =>
        needs.open({
            who: character.title,
            needs: wanted,
            fallback: character.voice.fallback?.title,
            retry: ({ install, anyway }) => api.chooseCharacter(character.id, { install, anyway }),
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
            const state = result.voice?.state;
            const wanted = result.voice?.needs ?? character.voice.needs;
            if (state === 'needs' && wanted && !mayVoice) setNote({ text: tr.characters.spokenReserveRestricted });
            else if (state === 'needs' && wanted) {
                setNote({ text: tr.characters.spokenReserveNeeds });
                askModel(character, wanted);
            } else setNote({ text: result.voice?.pending ? tr.characters.chosenModelInstalling : tr.onbCharacter.chosen(character.title) });
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
        if (fake === character.id) return setFake(null);
        setFake(character.id);
        timer.current = setTimeout(() => setFake(null), FAKE_MS);
    };

    const list = view ? shown(view) : [];
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

            {!view ? (
                <p class="small onb-ch-p" role={error ? 'alert' : 'status'}>
                    {error ?? tr.onbCharacter.reading}
                </p>
            ) : list.length === 0 ? (
                <p class="small onb-ch-p">{tr.onbCharacter.none}</p>
            ) : (
                <div class="onb-ch-grid">
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
                                    <span class={`face ${face}${character.avatar ? ' onb-ch-has-img' : ''}`}>
                                        <FaceImage src={character.avatar} />
                                        <span class="face-line">
                                            {sample?.text ? tr.onbCharacter.quoted(sample.text) : character.description}
                                        </span>
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
                                    <span class="pcard-about">{character.description}</span>
                                </button>
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
        </div>
    );
}
