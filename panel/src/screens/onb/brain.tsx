// Первые шаги, шаг «Кто думает» (только компьютер) — доска Brain холста «Онбординг»: пять путей — DuoWave (наши модели,
// docs/plans/duowave-connect.md; первым и во всю ширину, виден всем с 22.11), OpenRouter, подписка ChatGPT, свой ключ,
// модель на этом компьютере, — под ними карточка пути, ниже «Готовлю слух» и серая строка.
// Шапку и кнопки рисует рамка (onb/nav.tsx). Всё настоящее: те же вызовы шлюза, что у «Мозга» (screens/brain.tsx) —
// добавить провайдера, войти, проверить, выбрать модели (логика — общий хук screens/brain-connect.ts, тот же у витрины
// стола); слух — useLocalAsr (screens/local-asr.tsx).

import { useState } from 'preact/hooks';

import type { ProviderView } from '../../api';
import { tr } from '../../i18n';
import { Path, SignInPath, accessLine, useBrainConnect } from '../brain-connect';
import { localAsrNeedsCard, localAsrPercent, useLocalAsr } from '../local-asr';
import { awayBackup, awayBehind } from './brain-home';
import { useOnboarding, useStepNav } from './nav';
import '../../ui/onboarding-canvas.css';
import '../../ui/onb-brain.css';


const MB = 1_000_000;


const IconOk = () => (
    <svg class="ico" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M5 12.5l4.5 4.5L19 7.5" />
    </svg>
);
const IconWarn = () => (
    <svg class="ico" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 7v6M12 16.5v.5" />
    </svg>
);

/** Ряд карточки «готово / жду / не нашёл»: знак и две строки, как «Вошли в OpenRouter» на доске. */
function Said({ mark, title, note }: { mark: 'ok' | 'warn' | 'busy'; title: string; note: string }) {
    return (
        <>
            {mark === 'busy' ? (
                <span class="mark mark-busy" aria-hidden="true" />
            ) : (
                <span class={`mark mark-${mark}`}>{mark === 'ok' ? <IconOk /> : <IconWarn />}</span>
            )}
            <span class="ob-brain-text" role={mark === 'busy' ? 'status' : undefined}>
                <span class="h3">{title}</span>
                <br />
                <span class="small">{note}</span>
            </span>
        </>
    );
}

/** Шаг «Кто думает» первых шагов: доска Brain. */
export function BrainStep() {
    const t = tr.onbBrain;
    const { next } = useOnboarding();
    const [ear] = useLocalAsr();
    const {
        view,
        path,
        choosePath,
        checking,
        problem,
        key,
        setKey,
        fields,
        setFields,
        keyPreset,
        keyFields,
        keyUnknown,
        keyUrl,
        setKeyUrl,
        keyModel,
        setKeyModel,
        keyReady,
        saveKey,
        local,
        findLocal,
        signIn,
        beginSignIn,
        paths,
        invite,
        inviteBusy,
        saveInvite,
        workingOn,
        answering,
        noBackup,
    } = useBrainConnect();
    /** Что вписано в поле кода приглашения. */
    const [inviteDraft, setInviteDraft] = useState('');
    useStepNav(answering ? {} : { secondary: { label: t.later, onClick: next } }, [answering]);

    // Облачная «Запасная» остаётся и за моделью на компьютере: тогда «вопросы не уходят из дома» неправда — и пометка
    // «без интернета» тоже.
    const backupAway = view ? awayBackup(view) : null;
    // С DuoWave «Проще всего» — у него, у OpenRouter — «Много моделей»; без него доска прежняя.
    const card: Record<Path, { title: string; chip?: { text: string; lime?: boolean }; sub: string; wide?: boolean }> = {
        duowave: { title: t.dwTitle, chip: { text: t.dwChip, lime: true }, sub: t.dwSub, wide: true },
        openrouter: { title: t.orTitle, chip: { text: t.orChipMany }, sub: t.orSub },
        chatgpt: { title: t.gptTitle, sub: t.gptSub },
        key: { title: t.keyTitle, sub: t.keySub },
        local: { title: t.localTitle, chip: backupAway ? undefined : { text: t.localChip }, sub: backupAway ? t.localSubAway : t.localSub },
    };

    const working = workingOn(path);
    const busy = checking?.path === path ? checking : null;
    const said = problem?.path === path ? problem.text : null;

    /** Вход через сервис: кнопка и строка доски, под ними шаги входа (ссылка, код с другого устройства). */
    const signInBody = (which: SignInPath, label: string, note: string) => (
        <>
            <button
                type="button"
                class="dw-btn dw-btn-lg dw-btn-dark"
                aria-disabled={!view || signIn.starting ? 'true' : undefined}
                onClick={() => beginSignIn(which)}
            >
                {signIn.starting && signIn.target === which ? t.opening : label}
            </button>
            <span class="small ob-brain-text">{note}</span>
            {signIn.target === which && signIn.steps && <div class="ob-brain-full ob-brain-steps">{signIn.steps}</div>}
        </>
    );

    /**
     * «Нашёл Ollama»: кто отвечает и уходят ли вопросы из дома. За моделью на компьютере стоит облачная умная или
     * «Запасная» (её шлюз ставит и сам) — карточка называет её вместо «не уходят из дома»; при облачной «Запасной» под
     * ней — строка шлюза (может стоить денег, папки закрыты) и «Без запасной».
     */
    const localFound = (provider: ProviderView) => {
        const fast = view?.choice.fast;
        const answers = fast?.provider === provider.id;
        const model = answers && fast ? fast.model : (provider.models[0] ?? provider.title);
        const away = answers && view ? awayBehind(view) : null;
        const backup = answers && view ? awayBackup(view) : null;
        const gatewayNote = backup && view?.note?.kind === 'backupAuto' ? view.note.text : null;
        const note = !away ? t.localFoundNote(model) : away.backup ? t.localFoundBackup(model, away.title) : t.localFoundSmart(model, away.title);
        return (
            <>
                <Said mark="ok" title={t.localFound(provider.title)} note={note} />
                {backup && (
                    <button
                        type="button"
                        class="dw-btn dw-btn-sm dw-btn-light"
                        style={{ marginLeft: 'auto', flex: 'none' }}
                        onClick={() => void noBackup()}
                    >
                        {t.noBackup}
                    </button>
                )}
                {gatewayNote && (
                    <p class="small ob-brain-full" role="status" style={{ margin: 0 }}>
                        {gatewayNote}
                    </p>
                )}
            </>
        );
    };

    /**
     * DuoWave: сначала код приглашения — до входа (владелец, 22.11: ушёл на сайт без кода и вошёл — подарка уже не дадут,
     * подарок только при самом первом входе). Есть код — «Применить», и вход — «Получить подарок»; кода нет —
     * «Подключить без кода» сразу открывает сайт. Шлюз запомнит код и приложит его ко входу.
     */
    const duowaveBody = invite ? (
        signInBody('duowave', t.dwGift, t.dwGiftNote)
    ) : (
        <>
            <span class="ob-brain-text ob-brain-full">
                <span class="h3">{t.dwInviteAsk}</span>
                <br />
                <span class="small">{t.dwInviteWhy}</span>
            </span>
            <form
                class="ob-brain-full ob-brain-invite"
                onSubmit={(e) => {
                    e.preventDefault();
                    if (inviteDraft.trim() && !inviteBusy) void saveInvite(inviteDraft).then((saved) => saved && setInviteDraft(''));
                }}
            >
                <div class="field ob-brain-key">
                    <label for="dw-invite">{t.dwInviteLabel}</label>
                    <input
                        id="dw-invite"
                        class="input"
                        autocomplete="off"
                        spellcheck={false}
                        placeholder={t.dwInvitePlaceholder}
                        value={inviteDraft}
                        onInput={(e) => setInviteDraft((e.target as HTMLInputElement).value)}
                    />
                </div>
                <button type="submit" class="dw-btn dw-btn-lg dw-btn-dark" aria-disabled={!inviteDraft.trim() || inviteBusy ? 'true' : undefined}>
                    {t.dwInviteSave}
                </button>
            </form>
            <div class="ob-brain-full ob-brain-nocode">
                <button
                    type="button"
                    class="dw-btn dw-btn-sm dw-btn-light"
                    aria-disabled={!view || signIn.starting ? 'true' : undefined}
                    onClick={() => beginSignIn('duowave')}
                >
                    {signIn.starting && signIn.target === 'duowave' ? t.opening : t.dwNoCode}
                </button>
                <span class="small">{t.dwSignInNote}</span>
            </div>
            {signIn.target === 'duowave' && signIn.steps && <div class="ob-brain-full ob-brain-steps">{signIn.steps}</div>}
        </>
    );

    const body = busy ? (
        <Said mark="busy" title={t.checking(busy.title)} note={t.checkingNote} />
    ) : path === 'duowave' ? (
        working ? (
            <Said mark="ok" title={t.dwDone} note={working.access ? t.dwDoneNote(accessLine(working.access)) : t.chosenNote} />
        ) : (
            duowaveBody
        )
    ) : path === 'openrouter' ? (
        working ? <Said mark="ok" title={t.orDone} note={t.chosenNote} /> : signInBody('openrouter', t.orSignIn, t.orSignInNote)
    ) : path === 'chatgpt' ? (
        working ? <Said mark="ok" title={t.gptDone} note={t.chosenNote} /> : signInBody('chatgpt', t.gptSignIn, t.gptSignInNote)
    ) : path === 'key' ? (
        working ? (
            <Said mark="ok" title={t.keyDone(working.title)} note={t.chosenNote} />
        ) : (
            <form
                style={{ display: 'contents' }}
                onSubmit={(e) => {
                    e.preventDefault();
                    if (view && keyReady) void saveKey();
                }}
            >
                <div class="field ob-brain-key">
                    <label for="apikey">{t.keyLabel}</label>
                    <input
                        id="apikey"
                        class="input"
                        type="password"
                        placeholder={t.keyPlaceholder}
                        autocomplete="off"
                        value={key}
                        onInput={(e) => setKey((e.target as HTMLInputElement).value)}
                    />
                </div>
                {keyFields.map((field) => (
                    <div key={field.id} class="field ob-brain-key">
                        <label for={`apikey-${field.id}`}>{field.title}</label>
                        <input
                            id={`apikey-${field.id}`}
                            class="input"
                            autocomplete="off"
                            placeholder={field.placeholder}
                            value={fields[field.id] ?? ''}
                            onInput={(e) => setFields((all) => ({ ...all, [field.id]: (e.target as HTMLInputElement).value }))}
                        />
                    </div>
                ))}
                {/* Сервис по ключу не узнан — его адрес и, если нужно, модель: провайдер добавится отсюда же. */}
                {keyUnknown && (
                    <>
                        <div class="field ob-brain-key">
                            <label for="apikey-url">{t.keyUrlLabel}</label>
                            <input
                                id="apikey-url"
                                class="input"
                                type="url"
                                inputMode="url"
                                autocomplete="off"
                                placeholder={t.keyUrlPlaceholder}
                                value={keyUrl}
                                onInput={(e) => setKeyUrl((e.target as HTMLInputElement).value)}
                            />
                        </div>
                        <div class="field ob-brain-key">
                            <label for="apikey-model">{t.keyModelLabel}</label>
                            <input
                                id="apikey-model"
                                class="input"
                                autocomplete="off"
                                placeholder={t.keyModelPlaceholder}
                                value={keyModel}
                                onInput={(e) => setKeyModel((e.target as HTMLInputElement).value)}
                            />
                        </div>
                    </>
                )}
                <button type="submit" class="dw-btn dw-btn-lg dw-btn-dark ob-brain-key-btn" aria-disabled={!view || !keyReady ? 'true' : undefined}>
                    {t.keyCheck}
                </button>
                <span class="small ob-brain-key-note">{keyPreset ? t.keyKnown(keyPreset.title) : keyUnknown ? t.keyUnknownNote : t.keyNote}</span>
            </form>
        )
    ) : working ? (
        localFound(working)
    ) : local === 'none' ? (
        <>
            <Said mark="warn" title={t.localNone} note={t.localNoneNote} />
            <button type="button" class="dw-btn dw-btn-sm dw-btn-light" style={{ marginLeft: 'auto', flex: 'none' }} onClick={() => void findLocal(true)}>
                {t.localAgain}
            </button>
        </>
    ) : (
        <Said mark="busy" title={t.localSearching} note={t.localSearchingNote} />
    );

    return (
        <div class="ob-brain">
            <div class="ob-brain-head">
                <h1 class="h2" id="onb-brain-h">
                    {t.title1}
                    <br />
                    {t.title2}
                </h1>
                <p class="lead">{t.lead}</p>
            </div>

            <div class="ob-brain-opts" role="group" aria-labelledby="onb-brain-h">
                {paths.map((which) => (
                    <button
                        key={which}
                        type="button"
                        class={`opt${card[which].wide ? ' opt-wide' : ''}${path === which ? ' on' : ''}`}
                        aria-pressed={path === which ? 'true' : 'false'}
                        onClick={() => choosePath(which)}
                    >
                        <span class="radio" />
                        <span class="opt-body">
                            <span class="opt-title">
                                {card[which].title}
                                {card[which].chip && <span class={`chip${card[which].chip.lime ? ' chip-lime' : ''}`}>{card[which].chip.text}</span>}
                            </span>
                            <span class="opt-sub">{card[which].sub}</span>
                        </span>
                    </button>
                ))}
            </div>

            <div class="card card-tight ob-brain-card">
                {body}
                {said && (
                    <p class="small ob-brain-full ob-brain-problem" role="alert">
                        {said}
                    </p>
                )}
            </div>

            <div class="ob-brain-foot">
                {localAsrNeedsCard(ear) && (
                    <div class="ear" role="status">
                        <span class={`dot ${ear.phase === 'failed' ? 'dot-warn' : 'dot-busy'}`} />
                        {ear.phase === 'failed' ? (
                            <span>
                                <span class="q">{t.earFailed}</span> · {ear.error ?? tr.localAsr.notStarted}
                            </span>
                        ) : (
                            <span>
                                <span class="q">{t.earTitle}</span> · {ear.phase === 'downloading' ? t.earDownloading : t.earLoading}
                            </span>
                        )}
                        {ear.phase === 'downloading' && (
                            <>
                                <span
                                    class="bar"
                                    role="progressbar"
                                    aria-label={t.earProgress}
                                    aria-valuemin={0}
                                    aria-valuemax={100}
                                    aria-valuenow={Math.round(localAsrPercent(ear))}
                                >
                                    <i style={{ width: `${localAsrPercent(ear)}%` }} />
                                </span>
                                <span>{t.earSize(Math.round(ear.bytes / MB), Math.round(ear.total / MB))}</span>
                            </>
                        )}
                        {ear.phase === 'loading' && (
                            <span class="bar bar-run" aria-hidden="true">
                                <i />
                            </span>
                        )}
                    </div>
                )}
                {!answering && <span class="small">{t.noModelNote}</span>}
            </div>
        </div>
    );
}
