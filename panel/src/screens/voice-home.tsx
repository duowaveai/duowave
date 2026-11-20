// «Настройки» → «Голос» и «Слух» (проверка начисто 19.11, первый абзац Bugs.md). Выбор голоса и моделей слуха и голоса
// уехал вглубь — «Устройства» → Mac → «Этот компьютер» → длинная страница, — и после первых шагов «всё выбрали по
// стандарту, а кастомизировать сложно». Решение владельца: два своих пункта слева, раздельно.
// - «Голос»: голос ждёт (вопрос владельцу), кто говорит сейчас — где и на какой модели, чей голос отвечает, модель голоса
//   персонажа, и голоса компьютера — «Свой голос» первым и крупно, готовые голоса, облачные.
// - «Слух»: кто слушает сейчас, слово-активатор, модели распознавания компьютера.
// Свои модели живут на компьютере-Турбо: в окне колонки (`?via=`) они — его страница фреймом (`?own=voice`,
// `?own=hearing`, helper-pc.tsx), иначе — экран моделей прямо здесь (screens/models.tsx, `page`).

import { useEffect, useState } from 'preact/hooks';

import { CharactersView, HomeVoice, HomeVoicePlan, WhereRow, api, can, problemOf } from '../api';
import { useHelpers } from '../helpers';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { defaultEngine, engineStatus, EnginePicker, usableEngines } from '../ui/voice-plan';
import { VIA } from '../via';
import { onComputer } from '../voice-input';
import { ask } from '../ui/confirm';
import { WaitingCard } from '../voice/pending-voice';
import { aliceOnly, chooseWord, removeWords, useCallWords } from './call-words';
import { OwnFrame } from './helper-pc';
import { HomeVoiceChoice, ModelsScreen } from './models';
import { SayField } from './say';

function Card({ id, title, sub, children }: { id: string; title: string; sub?: string; children: preact.ComponentChildren }) {
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`vh-${id}`}>
            <div class="section-head flex flex-col">
                <h2 id={`vh-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
                {sub && <span class="w-cap">{sub}</span>}
            </div>
            {children}
        </section>
    );
}

/** Кто даёт умение сейчас (`/api/v1/helpers` → `now`) и что о нём сказал помощник: модель, голос, языки. */
function useWhere(ability: 'voice' | 'hearing'): { row?: WhereRow; details?: { model?: string; voice?: string; languages?: string[] } } {
    const { view } = useHelpers();
    const row = view?.now.find((entry) => entry.ability === ability);
    const helper =
        row?.by === 'helper'
            ? view?.helpers.find((entry) => entry.id === row.helper)
            : row?.by === 'here'
              ? view?.helpers.find((entry) => entry.builtin)
              : undefined;
    return { row, details: helper?.abilities.find((entry) => entry.id === ability)?.details };
}

/** «на «Mac»» или «на этом компьютере». */
const whereWords = (row: WhereRow | undefined): string => (row?.by === 'here' ? tr.voiceHome.nowHere : tr.voiceHome.nowWhere(row?.name ?? row?.helper ?? ''));

// ─── Голос ───────────────────────────────────────────────────────────────────

/** «Сейчас говорит»: Алиса, облачный голос или голос компьютера — какой, где и на какой модели. */
function NowSpeaking() {
    const panel = usePanel();
    const [home, setHome] = useState<HomeVoice | null>(null);
    const { row, details } = useWhere('voice');
    useEffect(() => {
        let alive = true;
        api.homeVoice()
            .then((fresh) => alive && setHome(fresh))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [panel.version]);
    const t = tr.voiceHome;
    // Колонка выбрала Алису или облако — так и есть, кто бы ни давал голос; иначе — тот, кто даёт его сейчас.
    const chosen = home && (home.offered || home.cloud) ? home.chosen : null;
    let line: string;
    let sub: string | null = null;
    if (chosen === 'alice') line = t.nowAlice;
    else if (chosen === 'cloud' || row?.by === 'cloud') line = t.nowCloud;
    else if (row?.by === 'helper' || row?.by === 'here') {
        const voice = details?.voice ?? (chosen === 'computer' ? home?.voice : undefined);
        line = voice ? t.nowVoice(voice, whereWords(row)) : t.nowComputer(row.name ?? row.helper ?? '');
        sub = details?.model ? t.nowModel(details.model) : null;
    } else if (row?.by === 'yandex') line = t.nowAlice;
    else line = t.nowNone;
    return (
        <Card id="now" title={t.nowTitle}>
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong wrap-anywhere">{line}</span>
                    {sub && <span class="w-cap">{sub}</span>}
                </span>
            </div>
        </Card>
    );
}

/**
 * Модель голоса персонажа на Турбо — тот же выбор, что в мастере установки (ui/voice-plan.tsx), но без мастера: выбрать
 * другую модель и «Перевести на …». Центр помнит выбор и говорит Турбо — тот ставит модель и включает голос сам.
 */
function CharacterVoiceModel() {
    const panel = usePanel();
    const [characters, setCharacters] = useState<CharactersView | null>(null);
    const [plan, setPlan] = useState<HomeVoicePlan | null>(null);
    const [picked, setPicked] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    useEffect(() => {
        let alive = true;
        api.characters()
            .then((fresh) => alive && setCharacters(fresh))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [panel.version]);
    const id = characters?.active?.id ?? null;
    const character = characters?.characters.find((entry) => entry.id === id);
    // Встроенным (Алиса, ДиВи) своего голоса на компьютере нет — выбирать нечего.
    const own = !!character && !character.builtin && !!character.package;
    useEffect(() => {
        if (!own || !id) return;
        let alive = true;
        const read = (): void =>
            void api
                .homeVoicePlan({ character: id })
                .then((fresh) => alive && setPlan(fresh))
                .catch(() => alive && setPlan(null));
        read();
        const timer = setInterval(read, 15_000);
        return () => {
            alive = false;
            clearInterval(timer);
        };
    }, [own, id]);
    if (!own || !character || !plan) return null;
    const t = tr.voiceHome;
    const choices = usableEngines(plan);
    const computer = plan.plan?.computer ?? plan.turbo?.name ?? '';
    // «Сейчас»: что правда ставится для голоса персонажа — Турбо называет её в `plan.chosen` (21.11), не желание.
    const current = plan.plan?.chosen ?? plan.chosen ?? null;
    const engine = picked ?? current ?? defaultEngine(plan);
    const currentRow = choices.find((choice) => choice.id === current);
    const pickedRow = choices.find((choice) => choice.id === engine);
    const apply = async (): Promise<void> => {
        if (!engine || !id) return;
        setBusy(true);
        setNote(null);
        try {
            setPlan(await api.setHomeVoiceEngine({ character: id }, engine));
            setPicked(null);
            setNote({ ok: true, text: t.modelApplied });
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    return (
        <Card id="model" title={t.modelTitle(character.title)} sub={computer ? t.modelSub(computer) : undefined}>
            {!plan.plan ? (
                <p class="w-cap m-0">{t.modelAway}</p>
            ) : choices.length === 0 ? (
                <p class="w-cap m-0">{t.modelNone}</p>
            ) : (
                <>
                    {currentRow && <p class="w-cap m-0">{t.modelNow(currentRow.title, engineStatus(currentRow))}</p>}
                    <EnginePicker plan={plan} engine={engine} onSelect={(next) => setPicked(next)} />
                    {can('models.manage') && pickedRow && engine !== current && (
                        <div class="w-actions">
                            <button class="w-btn w-btn-ink" disabled={busy} aria-busy={busy || undefined} onClick={() => void apply()}>
                                {t.modelApply(pickedRow.title)}
                            </button>
                        </div>
                    )}
                </>
            )}
            {note && (
                <p class={`w-cap m-0${note.ok ? '' : ' c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </p>
            )}
        </Card>
    );
}

/** Голоса компьютера: в окне колонки — его страницей фреймом, иначе — экран моделей прямо здесь. */
function ComputerModels({ kind }: { kind: 'voice' | 'hearing' }) {
    if (VIA) return <OwnFrame page={kind} title={kind === 'voice' ? tr.voiceHome.computerVoices : tr.voiceHome.computerHearing} />;
    return <ModelsScreen tab={kind === 'voice' ? 'voice' : 'recognition'} onTab={() => undefined} bare page />;
}

/** «Настройки» → «Голос». */
export function VoicePage() {
    return (
        <>
            <p class="w-cap m-0">{tr.voiceHome.voiceLead}</p>
            <WaitingCard />
            <NowSpeaking />
            {/* «Сказать» (владелец, 20.11): свой текст голосом персонажа или Алисой — колонкой, без неё компьютером. */}
            <Card id="say" title={tr.say.cardTitle} sub={tr.say.cardSub}>
                <SayField />
            </Card>
            <HomeVoiceChoice />
            <CharacterVoiceModel />
            <ComputerModels kind="voice" />
        </>
    );
}

// ─── Слух ────────────────────────────────────────────────────────────────────

/** «Сейчас слушает»: Яндекс, облако или модель компьютера — где и какие языки. */
function NowHearing() {
    const panel = usePanel();
    const { row, details } = useWhere('hearing');
    const t = tr.voiceHome;
    const relay = panel.health?.panelMode === 'relay';
    let line: string;
    let sub: string | null = null;
    if (relay || row?.by === 'yandex') line = t.hearYandex;
    else if (row?.by === 'cloud') line = t.hearCloud;
    else if (row?.by === 'helper' || row?.by === 'here') {
        line = details?.model ? t.hearModel(details.model, whereWords(row)) : t.hearComputer(whereWords(row));
        sub = details?.languages?.length ? t.hearLanguages(details.languages.join(', ')) : null;
    } else line = t.hearNone;
    return (
        <Card id="hear-now" title={t.hearNowTitle}>
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong wrap-anywhere">{line}</span>
                    {sub && <span class="w-cap">{sub}</span>}
                </span>
            </div>
        </Card>
    );
}

/**
 * Слово-активатор (замечание 11 теста 19.11): все слова, что стоят, — какое слушает колонка, «Включить» другое (прежнее
 * выключится: слушает одно), «Выключить», «Убрать»; сверху — «Только Алиса». Своё слово приходит парой к персонажу
 * (magicplace/filters.ts); нет ни одного — в магазин, к словам.
 */
function CallWordCard() {
    const panel = usePanel();
    const { words, reload, station } = useCallWords();
    const t = tr.voiceHome;
    const fallback = onComputer() ? tr.core.assistantComputer : tr.wizard.alice;
    const mayManage = can('plugins.manage');
    const [busy, setBusy] = useState<string | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const on = words?.find((word) => word.enabled);
    const run = async (key: string, job: () => Promise<void>): Promise<void> => {
        setBusy(key);
        setProblem(null);
        try {
            await job();
        } catch (e) {
            setProblem(problemOf(e));
        } finally {
            setBusy(null);
            reload();
        }
    };
    const removeAll = async (): Promise<void> => {
        if (!words?.length) return;
        const yes = await ask({ title: t.wordRemoveAllTitle, text: t.wordRemoveAllText, ok: t.wordRemoveAll, danger: true });
        if (yes) await run('remove', () => removeWords(station, words));
    };
    return (
        <Card id="word" title={t.wordTitle}>
            <div class="w-row">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{on ? t.wordIs(on.word) : t.wordDefault(fallback)}</span>
                    <span class="w-cap">{words?.length ? t.wordAboutMany : t.wordAbout}</span>
                </span>
                {mayManage && on && (
                    <button
                        class="w-btn"
                        disabled={busy !== null}
                        aria-busy={busy === 'alice' || undefined}
                        onClick={() => void run('alice', () => aliceOnly(station, words ?? []))}
                    >
                        {t.wordAliceOnly(fallback)}
                    </button>
                )}
            </div>
            {(words ?? []).map((word) => (
                <div key={word.id} class="w-row">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">«{word.word}»</span>
                        <span class="w-cap">{word.enabled ? t.wordListening : t.wordOff}</span>
                    </span>
                    {mayManage &&
                        (word.enabled ? (
                            <button
                                class="w-btn"
                                disabled={busy !== null}
                                aria-busy={busy === word.id || undefined}
                                onClick={() => void run(word.id, () => aliceOnly(station, [word]))}
                            >
                                {t.wordTurnOff}
                            </button>
                        ) : (
                            <button
                                class="w-btn"
                                disabled={busy !== null}
                                aria-busy={busy === word.id || undefined}
                                onClick={() => void run(word.id, () => chooseWord(station, words ?? [], word.id))}
                            >
                                {t.wordTurnOn}
                            </button>
                        ))}
                </div>
            ))}
            <div class="w-row">
                {mayManage && (
                    <button class="w-btn" onClick={() => panel.go('magicplace', 'store', undefined, { kind: 'word' })}>
                        {t.wordFind}
                    </button>
                )}
                {mayManage && !!words?.length && (
                    <button
                        class="w-btn"
                        disabled={busy !== null}
                        aria-busy={busy === 'remove' || undefined}
                        onClick={() => void removeAll()}
                    >
                        {t.wordRemoveAll}
                    </button>
                )}
                {can('persona.edit') && (
                    <button class="w-btn" onClick={() => panel.go('character', 'characters')}>
                        {t.wordOpen}
                    </button>
                )}
            </div>
            {problem && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {problem}
                </p>
            )}
        </Card>
    );
}

/** «Настройки» → «Слух». */
export function HearingPage() {
    return (
        <>
            <p class="w-cap m-0">{tr.voiceHome.hearingLead}</p>
            <NowHearing />
            <CallWordCard />
            <ComputerModels kind="hearing" />
        </>
    );
}
