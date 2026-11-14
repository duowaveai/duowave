// «Примерка» (docs/plans/voice-creation.md, «Пульт: плитка, комета, «Примерка», посадка»; макет docs/design/voice-creation,
// шаги «Примерка», «Личное», «Ваше «да»», «На столе»). Окно собирает готовые части в одно место: та же плитка на кусочке
// стола (стенд) с примером данных — её место на столе ждёт пунктиром; фразы для колонки — нажать или сказать, ответ из
// примера; «Что ему нужно» — обычное можно сказать, личное только руками (пока поле открыто, микрофон выключен); «Что
// ему можно» — права, янтарь — то, что уходит из дома. Внизу — «Переделать…», «В черновики», «Только фразы» и лаймовая
// «На стол»; пустой обязательный ключ — вместо неё «Вставить ключ». Первую установку и новые права одобряют рукой:
// янтарная плашка «Станет: …» и «Одобрить и поставить · Enter» (голос можно подделать). Без новых прав «На стол» ставит
// сразу. Поставили — плитка садится на своё место настоящим листом с меткой «новый», и только теперь в ней ваши данные.

import { useEffect, useRef, useState } from 'preact/hooks';

import { DraftView, InstallPlan, InstallReport, PhraseReport, api, can, problemOf } from '../api';
import { IconLock, IconMicOff, IconPlay } from '../icons';
import { usePanel } from '../shell/context';
import { registerPlugin, unregisterPlugin } from '../shell/registry';
import type { SlotContribution } from '../shell/slot';
import { setTheme, useTheme } from '../shell/theme';
import { normalizeTheme } from '../shell/themes';
import { deskActions } from '../shell/widgets';
import { ask } from '../ui/confirm';
import { PermissionLines } from '../ui/rights-lines';
import { SettingInput } from '../ui/settings-fields';
import { announceLanded, askToMake, failed, missingKey, needsYes } from '../voice/creation';
import { onWorkshopChange, useAgentPlace } from '../voice/drafts';
import { useMicHold, useVoiceAction } from '../voice/phrases';
import { hideStrip, stripNow } from '../voice/strip';
import { placeOnCanvas } from './desk-canvas';
import { DraftWidgetPreview, madeByVoice, mayRunDraft, trustOwnDraft, trustedCode } from './draft-preview';
import { DraftCardPreview } from '../cards/draft-card';
import { faceWord } from './draft-tile';
import { formatList, tr } from '../i18n';
import { byMode } from '../words';

/** Ширина плитки на столе: так она встанет. */
const WIDTHS = { narrow: 240, normal: 300, wide: 420 } as const;
type Width = keyof typeof WIDTHS;
const widthWords = (): Record<Width, string> => ({ narrow: tr.fitting.widthNarrow, normal: tr.fitting.widthNormal, wide: tr.fitting.widthWide });
/** Тема на примерке — своя запись в реестре пульта, как у мастерской. */
const THEME_TRY = 'fitting.theme';

/** Фраза манифеста, как её пробуют: «добавь дело {task}» → «добавь дело пример». */
const sample = (template: string): string => template.replace(/\{[^}]+\}/g, 'пример').trim();
/** Фраза для людей: «добавь дело …». */
const shown = (template: string): string => template.replace(/\{[^}]+\}/g, '…').trim();

/** Что станет — словами плашки «Станет: …». */
function willBe(draft: DraftView, plan: InstallPlan): string {
    const parts: string[] = [];
    if (draft.meta.kind === 'widget') parts.push(tr.fitting.willWidget);
    else if (draft.meta.kind === 'theme') parts.push(tr.fitting.willTheme);
    else parts.push(draft.meta.kind === 'game' ? byMode(tr.fitting.willGameSpeaker, tr.fitting.willGameVoice) : byMode(tr.fitting.willSkillSpeaker, tr.fitting.willSkillVoice));
    if (plan.phrases.length) parts.push(byMode(tr.fitting.phrasesSpeaker, tr.fitting.phrasesVoice)(plan.phrases.length));
    const hosts = plan.hostsAdded.length ? plan.hostsAdded : [];
    const goes = hosts.length ? tr.fitting.willGoOnline(hosts.join(', ')) : '';
    const first = plan.first ? '' : plan.from ? tr.fitting.willVersion(plan.from, plan.to) : '';
    return `${formatList(parts)}.${goes}${first}`;
}

export function FittingSection() {
    const panel = usePanel();
    const id = panel.link.section === 'fitting' ? panel.link.tab : undefined;
    if (!can('plugins.manage')) return <p class="t-caption m-0">{tr.fitting.needManage}</p>;
    if (!id) return <p class="t-caption m-0">{tr.fitting.nothingToTry}</p>;
    return <Fitting key={id} id={id} yes={'yes' in panel.link.query} redo={'redo' in panel.link.query} />;
}

function Fitting({ id, yes, redo }: { id: string; yes: boolean; redo: boolean }) {
    const panel = usePanel();
    const role = panel.role.role;
    const cloud = useAgentPlace();
    const { theme } = useTheme();
    const [draft, setDraft] = useState<DraftView | null>(null);
    const [plan, setPlan] = useState<InstallPlan | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [values, setValues] = useState<Record<string, unknown>>({});
    const [privateOpen, setPrivateOpen] = useState(false);
    const [consent, setConsent] = useState(false);
    const [busy, setBusy] = useState<string | null>(null);
    const [trouble, setTrouble] = useState<string | null>(null);
    const [width, setWidth] = useState<Width>('normal');
    // У приложения есть окно (раздел): «Развернуть» показывает его на том же примере (docs/plans/midi.md).
    const [hasWindow, setHasWindow] = useState(false);
    const [asWindow, setAsWindow] = useState(false);
    const [code, setCode] = useState<string | null>(null);
    const [widget, setWidget] = useState<SlotContribution | null>(null);
    const [tried, setTried] = useState<Record<string, PhraseReport | string>>({});
    const [redoOpen, setRedoOpen] = useState(redo);
    const [redoWords, setRedoWords] = useState('');
    const [themeBefore, setThemeBefore] = useState<string | null>(null);
    const [version, setVersion] = useState(0);
    const keyField = useRef<HTMLDivElement>(null);
    const askedYes = useRef(false);
    // Личное — только руками: пока поле ключа открыто, пульт не слушает.
    useMicHold(privateOpen);

    // Черновик и что изменит установка; перечитать, когда агент его поменял.
    useEffect(() => onWorkshopChange((draftId, change) => draftId === id && change !== 'agent' && setVersion((n) => n + 1)), [id]);
    useEffect(() => {
        let alive = true;
        api.workshopDraft(id)
            .then((view) => {
                if (!alive) return;
                setDraft(view);
                setValues((before) => {
                    const next = { ...before };
                    for (const need of view.needs) if (!(need.key in next)) next[need.key] = need.secret ? '' : (need.default ?? null);
                    return next;
                });
            })
            .catch((e) => alive && setProblem(problemOf(e)));
        api.installPlan(id)
            .then((answer) => alive && setPlan(answer))
            .catch((e) => alive && setProblem(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [id, version]);
    // Код виджета работает в своём окошке, без прав пульта (docs/plans/bus.md, «Окошки»): показ без вопроса — и
    // у черновика, попрошенного у колонки.
    useEffect(() => {
        if (!draft || !draft.files.some((file) => file.path === 'panel.js')) return;
        void trustedCode(id).then((known) => known ?? trustOwnDraft(id)).then(setCode);
    }, [draft?.meta.updated]);
    // Тема на примерке уходит вместе с окном. «Готово…» на полосе своё сказало: окно открыто.
    useEffect(() => {
        if (stripNow()?.kind === 'ready') hideStrip();
        return () => {
            unregisterPlugin(THEME_TRY);
        };
    }, []);

    const plain = draft?.needs.filter((need) => !need.secret) ?? [];
    const secret = draft?.needs.filter((need) => need.secret) ?? [];
    /** Обычное — в пример (песочница); личное туда не идёт никогда. */
    const plainValues = Object.fromEntries(plain.map((need) => [need.key, values[need.key]]).filter(([, value]) => value !== null && value !== ''));
    const keyMissing = draft ? missingKey(draft, values) : false;
    const title = draft?.title?.trim() || id;
    const word = draft ? faceWord(draft) : tr.fitting.thing;

    const act = async (name: string, run: () => Promise<unknown>) => {
        setBusy(name);
        setTrouble(null);
        try {
            await run();
        } catch (e) {
            setTrouble(failed(e));
        } finally {
            setBusy(null);
        }
    };

    /** Посадка: на стол на место плитки, метка «новый»; полоса — что колонка теперь знает. */
    const land = (report: InstallReport) => {
        if (!draft || !plan) return;
        if (draft.meta.kind === 'widget') {
            const key = `package.${id}.${widget?.id ?? 'main'}`;
            const spot = placeOnCanvas(document.querySelector('.draft-tile, .draft-slot'));
            deskActions(role).land(key, spot ? { ...spot, w: WIDTHS[width] } : undefined);
        }
        announceLanded(title, plan.phrases.map(shown), {
            remove: () => void api.removePackage(id).catch(() => undefined),
            redo: () => panel.go('skills', 'workshop', id),
            again: () => panel.openAdd(),
        });
        void report;
        panel.go('home');
    };

    /** Поставить: настройки — с установкой, до первого запуска; новые права — только с `approve` (нажатие). */
    const install = (approve: boolean, enable = true) =>
        act('install', async () => {
            const settings = Object.fromEntries(Object.entries(values).filter(([, value]) => value !== null && value !== ''));
            const report = await api.installDraft(id, approve, settings);
            if (!report.installed) {
                if (report.needsApproval) setConsent(true);
                else throw new Error(report.message);
                return;
            }
            if (!enable && report.package?.enabled) await api.disablePackage(id).catch(() => undefined);
            land(report);
        });

    /** «На стол»: нет ключа — к полю ключа; нужно «да» — плашка «Станет…»; иначе — сразу. */
    const toDesk = () => {
        if (!draft || !plan) return;
        if (keyMissing) {
            // После окна: оно при открытии берёт фокус себе.
            window.setTimeout(() => keyField.current?.querySelector('input')?.focus(), 60);
            return;
        }
        if (needsYes(plan)) setConsent(true);
        else void install(false);
    };
    // Открыли «На стол» с плитки, полосы или колонки: окно сразу у «да» (или у ключа).
    useEffect(() => {
        if (!yes || askedYes.current || !draft || !plan) return;
        askedYes.current = true;
        toDesk();
    }, [yes, !!draft, !!plan]);
    // «Одобрить и поставить · Enter»: только нажатием.
    useEffect(() => {
        if (!consent) return;
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== 'Enter' || event.isComposing || busy) return;
            const target = event.target as HTMLElement | null;
            if (target?.closest('textarea, input:not([type="checkbox"])')) return;
            event.preventDefault();
            void install(true);
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [consent, busy, values, width, widget]);

    const tryPhrase = (template: string) =>
        act(`phrase:${template}`, async () => {
            const report = await api.tryDraftPhrase(id, sample(template), { example: true, settings: plainValues });
            setTried((before) => ({ ...before, [template]: report }));
        });
    const park = () => act('park', () => api.parkDraft(id).then(() => panel.go('home')));
    const again = (words: string) =>
        act('redo', async () => {
            await askToMake(words.trim() || tr.fitting.redoDefault, cloud, id);
            setRedoOpen(false);
            setRedoWords('');
            panel.go('home');
        });
    const dropDraft = () =>
        void ask({ title: tr.fitting.dropTitle, text: tr.fitting.dropText(title), ok: tr.fitting.dropOk, danger: true }).then(
            (sure) => {
                if (sure) void act('drop', () => api.deleteDraft(id).then(() => panel.go('home')));
            },
        );
    const tryTheme = () =>
        act('theme', async () => {
            const found = normalizeTheme(JSON.parse((await api.readDraftFile(id, 'manifest.json')).content)?.contributes?.themes?.[0]);
            if (!found) throw new Error(tr.fitting.noThemeInManifest);
            const before = themeBefore ?? theme.id;
            registerPlugin({ id: THEME_TRY, themes: [{ ...found, id: `fitting-${found.id}`, title: tr.fitting.themeTryTitle(found.title) }] });
            setTheme(`fitting-${found.id}`);
            setThemeBefore(before);
        });
    const untryTheme = () => {
        if (!themeBefore) return;
        unregisterPlugin(THEME_TRY);
        setTheme(themeBefore);
        setThemeBefore(null);
    };

    // Что на кнопках — то и говорите. «Одобрить и поставить» и поле ключа голосу недоступны.
    const ready = !!draft && !!plan && !consent;
    useVoiceAction(ready && !keyMissing ? ['на стол', 'поставь'] : [], toDesk);
    useVoiceAction(ready ? ['в черновики'] : [], () => void park());
    useVoiceAction(ready ? ['переделай', 'переделать'] : [], (rest) => (rest ? void again(rest) : setRedoOpen(true)));
    useVoiceAction(ready && !keyMissing ? ['только фразы'] : [], () => (plan && needsYes(plan) ? setConsent(true) : void install(false, true)));
    useVoiceAction(ready ? ['попробуй', 'попробовать'] : [], () => plan?.phrases[0] && void tryPhrase(plan.phrases[0]));
    useVoiceAction(ready ? ['узкий', 'сделай уже'] : [], () => setWidth('narrow'), false);
    useVoiceAction(ready ? ['обычный'] : [], () => setWidth('normal'), false);
    useVoiceAction(ready ? ['широкий', 'сделай шире'] : [], () => setWidth('wide'), false);
    useVoiceAction(consent ? ['отменить', 'отмена'] : [], () => setConsent(false));
    // Обычные настройки голосом: «проект — Дом», «показывай три», — по названию настройки (основа его первого слова).
    const stems = plain.map((need) => {
        const first = need.title.toLowerCase().split(/\s+/)[0] ?? '';
        return { need, stem: first.slice(0, Math.max(4, first.length - 3)) };
    });
    useVoiceAction(ready ? stems.map((entry) => `~${entry.stem}`) : [], (rest, phrase) => {
        const need = stems.find((entry) => phrase.startsWith(entry.stem))?.need;
        if (!need || !rest) return;
        const value =
            need.type === 'number'
                ? Number(rest.replace(/[^\d]/g, '')) || wordNumber(rest)
                : need.options
                  ? need.options.find((option) => option.toLowerCase().startsWith(rest))
                  : rest;
        if (value !== undefined && value !== null && value !== '') setValues((before) => ({ ...before, [need.key]: value }));
    });

    if (problem && !draft) return <p class="t-caption c-dot-offline m-0">{problem}</p>;
    if (!draft || !plan) return <p class="t-caption m-0">{tr.fitting.openingDraft}</p>;

    const face = draft.meta.kind;
    const asked = draft.meta.by === 'voice' ? byMode(tr.fitting.askedSpeaker, tr.fitting.askedVoice) : null;
    const seconds = draft.meta.stage === 'ready' ? null : draft.meta.stage;

    return (
        <div class="fitting desk-window-wide">
            <div class="fitting-chips">
                <span class="fit-chip">
                    <span class="fit-dot is-agent" aria-hidden="true" />
                    {tr.fitting.madeByAgent}{asked ? ` · ${asked}` : ''}
                </span>
                <span class="fit-chip">{tr.fitting.draftNothingInstalled}</span>
                {seconds && <span class="fit-chip">{seconds === 'working' ? tr.fitting.agentStillWorking : seconds}</span>}
                <span class="fitting-sp" />
                <button class="pill" onClick={() => panel.go('skills', 'workshop', id)}>
                    {tr.fitting.openCode}
                </button>
            </div>
            <div class="fitting-body">
                <div class="fitting-col">
                    <div>
                        <h3 class="fitting-title">{title}</h3>
                        <p class="t-caption m-0">{draft.meta.note ?? `${word[0].toUpperCase()}${word.slice(1)}: ${draft.meta.asked}`}</p>
                    </div>
                    <div class="fitting-stand">
                        <div class="fitting-stand-top">
                            {face === 'widget' && (
                                <div class="seg" role="group" aria-label={tr.fitting.widthLabel}>
                                    {(Object.keys(WIDTHS) as Width[]).map((entry) => (
                                        <button key={entry} aria-pressed={width === entry} onClick={() => setWidth(entry)}>
                                            {widthWords()[entry]}
                                        </button>
                                    ))}
                                </div>
                            )}
                            <span class="t-caption">{asWindow ? tr.fitting.standWindow : face === 'widget' ? tr.fitting.standWidget : tr.fitting.standSample}</span>
                            {hasWindow && (
                                <button class="pill" aria-pressed={asWindow} onClick={() => setAsWindow(!asWindow)}>
                                    {asWindow ? tr.fitting.collapse : tr.fitting.expand}
                                </button>
                            )}
                        </div>
                        <div class="fitting-tile" style={{ width: asWindow ? '100%' : `${face === 'widget' ? WIDTHS[width] : 340}px` }}>
                            {face === 'theme' ? (
                                <div class="flex flex-col gap-8">
                                    <p class="t-caption m-0">{tr.fitting.themeVisible}</p>
                                    {themeBefore ? (
                                        <button class="pill" onClick={untryTheme}>
                                            {tr.fitting.untryTheme}
                                        </button>
                                    ) : (
                                        <button class="pill" disabled={busy !== null} onClick={() => void tryTheme()}>
                                            {tr.fitting.tryTheme}
                                        </button>
                                    )}
                                </div>
                            ) : code ? (
                                <DraftWidgetPreview
                                    id={id}
                                    nonce={draft.meta.updated + version}
                                    code={code}
                                    trying={{ example: true, settings: plainValues }}
                                    onWidget={setWidget}
                                    window={asWindow}
                                    onWindow={setHasWindow}
                                />
                            ) : draft.files.some((file) => file.path === 'panel.js') ? (
                                <div class="draft-ask">
                                    <p class="t-caption m-0">{asked ? byMode(tr.fitting.askedSpeakerCode, tr.fitting.askedVoiceCode) : tr.fitting.loading}</p>
                                    {asked && (
                                        <button class="pill" onClick={() => void mayRunDraft(id).then(setCode)}>
                                            {tr.fitting.show}
                                        </button>
                                    )}
                                </div>
                            ) : (
                                <DraftCardPreview
                                    id={id}
                                    nonce={draft.meta.updated + version}
                                    trying={{ example: true, settings: plainValues }}
                                    onWidget={setWidget}
                                    fallback={<p class="t-caption m-0">{(draft.meta.kind === 'theme' ? tr.fitting.noWidgetTheme : tr.fitting.noWidgetThing)(byMode(tr.fitting.onlyPhrasesSpeaker, tr.fitting.onlyPhrasesVoice))}</p>}
                                />
                            )}
                        </div>
                        <span class="fitting-note t-caption">{tr.fitting.sampleNote}</span>
                    </div>
                    {plan.phrases.length > 0 && (
                        <>
                            <div class="fitting-h">
                                {tr.fitting.hears} <span class="t-caption">{tr.fitting.hearsHint}</span>
                            </div>
                            <div class="fitting-phrases">
                                {plan.phrases.map((template) => {
                                    const answer = tried[template];
                                    const said = typeof answer === 'string' ? answer : (answer?.speech ?? answer?.error ?? (answer?.matched ? tr.fitting.answeredNoSpeech : answer ? tr.fitting.unknownPhrase : null));
                                    return (
                                        <div key={template} class="fitting-phrase">
                                            <button class="fitting-play" aria-label={tr.fitting.tryPhrase(shown(template))} disabled={busy !== null} onClick={() => void tryPhrase(template)}>
                                                <IconPlay size={10} />
                                            </button>
                                            «{shown(template)}»
                                            {said && <span class="fitting-answer">«{said}»</span>}
                                        </div>
                                    );
                                })}
                            </div>
                        </>
                    )}
                </div>
                <div class="fitting-col">
                    <div class="fitting-block">
                        <div class="fitting-h">{tr.fitting.needs}</div>
                        {draft.needs.length === 0 && <p class="t-caption m-0">{tr.fitting.needsNothing}</p>}
                        {plain.length > 0 && (
                            <>
                                <div class="fitting-group">
                                    <b>{tr.fitting.groupPlainBold}</b>{tr.fitting.groupPlainRest}
                                </div>
                                {plain.map((need) => (
                                    <label key={need.key} class="fitting-row">
                                        <span>{need.title}</span>
                                        <SettingInput field={need} value={values[need.key]} busy={busy !== null} onChange={(value) => setValues((before) => ({ ...before, [need.key]: value }))} />
                                    </label>
                                ))}
                            </>
                        )}
                        {secret.length > 0 && (
                            <>
                                {plain.length > 0 && <div class="fitting-sep" />}
                                <div class="fitting-group">
                                    <IconLock size={13} />
                                    <b>{tr.fitting.groupPrivateBold}</b>{tr.fitting.groupPrivateRest}
                                </div>
                                <div
                                    ref={keyField}
                                    class={`fitting-keys${privateOpen ? ' is-open' : ''}`}
                                    // Enter в поле ключа — «Готово»: поле закрывается, микрофон снова можно.
                                    onKeyDown={(event) => event.key === 'Enter' && (event.target as HTMLElement).blur()}
                                >
                                    {secret.map((need) => (
                                        <label key={need.key} class="fitting-row">
                                            <span>
                                                {need.title}
                                                {need.required ? '' : tr.fitting.optionalSuffix}
                                            </span>
                                            <SettingInput field={{ ...need, secret: true }} value={values[need.key]} busy={busy !== null} onChange={(value) => setValues((before) => ({ ...before, [need.key]: value }))} onPrivate={setPrivateOpen} />
                                        </label>
                                    ))}
                                </div>
                                <ul class="fitting-private">
                                    {privateOpen && (
                                        <li class="is-mic">
                                            <IconMicOff size={13} />
                                            {tr.fitting.micOffWhileKey}
                                        </li>
                                    )}
                                    <li>
                                        <span class="fit-dot is-own" aria-hidden="true" />
                                        {tr.fitting.staysInGateway}
                                    </li>
                                    <li>
                                        <span class="fit-dot is-own" aria-hidden="true" />
                                        {tr.fitting.agentCantSee}
                                    </li>
                                </ul>
                            </>
                        )}
                    </div>
                    <div class="fitting-block">
                        <div class="fitting-h">
                            {tr.fitting.allowed} <span class="t-caption">{tr.fitting.youApprove}</span>
                        </div>
                        <PermissionLines plan={plan} needs={draft.needs} />
                    </div>
                </div>
            </div>
            {redoOpen && !consent && (
                <form
                    class="fitting-redo"
                    onSubmit={(event) => {
                        event.preventDefault();
                        void again(redoWords);
                    }}
                >
                    <input class="field" value={redoWords} placeholder={tr.fitting.redoPlaceholder} aria-label={tr.fitting.redoLabel} onInput={(event) => setRedoWords((event.target as HTMLInputElement).value)} />
                    <button type="submit" class="pill" disabled={busy !== null}>
                        {busy === 'redo' ? tr.fitting.handingToAgent : tr.fitting.redo}
                    </button>
                </form>
            )}
            <footer class="fitting-foot">
                {consent ? (
                    <div class="fitting-will" role="group" aria-label={tr.fitting.approvalLabel}>
                        <p>
                            <b>{tr.fitting.willBeLabel}</b> {willBe(draft, plan)}{tr.fitting.approveByHand}
                        </p>
                        <button class="pill pill-lg" disabled={busy !== null} onClick={() => setConsent(false)}>
                            {tr.fitting.cancel}
                        </button>
                        <button class="pill pill-lg pill-own fitting-approve" disabled={busy !== null} autoFocus onClick={() => void install(true)}>
                            {busy === 'install' ? tr.fitting.installing : tr.fitting.approveAndInstall}
                            <span class="kbd">Enter</span>
                        </button>
                    </div>
                ) : privateOpen ? (
                    <div class="fitting-say">
                        <span class="strip-mic is-off" aria-hidden="true">
                            <IconMicOff size={18} />
                        </span>
                        <p>
                            <b>{tr.fitting.micOffBold}</b>{tr.fitting.keyByHand}
                        </p>
                        <button
                            class="pill pill-lg pill-ink"
                            // Нажатие не уводит фокус раньше времени: кнопка сама закрывает поле.
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => (document.activeElement as HTMLElement | null)?.blur()}
                        >
                            {tr.fitting.done}
                        </button>
                    </div>
                ) : (
                    <>
                        <div class="fitting-say">
                            <p>
                                {tr.fitting.sayButtons}{' '}
                                {keyMissing ? (
                                    <>
                                        {tr.fitting.keyNotAloud}<b>{tr.fitting.keyByHandBold}</b>.
                                    </>
                                ) : plain.length > 0 ? (
                                    <>
                                        {tr.fitting.orLike}<b>{tr.fitting.orLikeSay(plain[0].title.toLowerCase())}</b>, <b>{tr.fitting.orLikeWider}</b>.
                                    </>
                                ) : null}
                            </p>
                        </div>
                        <button class="pill pill-lg" disabled={busy !== null} onClick={() => setRedoOpen(!redoOpen)}>
                            {tr.fitting.redoEllipsis}
                        </button>
                        <button class="pill pill-lg" disabled={busy !== null} onClick={() => void park()}>
                            {tr.fitting.toDrafts}
                        </button>
                        {keyMissing ? (
                            <button class="pill pill-lg pill-own" onClick={toDesk}>
                                {tr.fitting.pasteKey}
                            </button>
                        ) : (
                            <>
                                {face === 'widget' && plan.phrases.length > 0 && (
                                    <button class="pill pill-lg pill-ink" disabled={busy !== null} onClick={() => (needsYes(plan) ? setConsent(true) : void install(false, true))}>
                                        {tr.fitting.onlyPhrases}
                                    </button>
                                )}
                                <button class="pill pill-lg pill-own" disabled={busy !== null} onClick={toDesk}>
                                    {busy === 'install' ? tr.fitting.installing : face === 'widget' ? tr.fitting.toDesk : tr.fitting.enable}
                                </button>
                            </>
                        )}
                    </>
                )}
            </footer>
            <div class="fitting-more">
                <button class="linkish" onClick={() => panel.go('skills', 'workshop', id)}>
                    {byMode(tr.fitting.tryOnSpeaker, tr.fitting.tryOnVoice)}
                </button>
                <button class="linkish" onClick={dropDraft}>
                    {tr.fitting.dropDraft}
                </button>
            </div>
            {trouble && (
                <p class="t-caption c-dot-offline m-0" role="alert">
                    {trouble}
                </p>
            )}
        </div>
    );
}

/** «три» → 3: числа словами, что говорят о настройках. */
function wordNumber(text: string): number | null {
    const words: Record<string, number> = { один: 1, одну: 1, два: 2, две: 2, три: 3, четыре: 4, пять: 5, шесть: 6, семь: 7, восемь: 8, девять: 9, десять: 10 };
    const found = text.split(/\s+/).find((word) => word in words);
    return found ? words[found] : null;
}
