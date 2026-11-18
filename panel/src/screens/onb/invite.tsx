// Первые шаги → «Персонаж» → «Есть приглашение?» (проверка начисто 18.11, п. 42; владелец: демо — по приглашению, лента
// не зашита в сборки). Человек вставляет ссылку из приглашения — адрес ленты, с `catalog.json` на конце или без; после
// одного честного предупреждения она становится своим источником у центра, как «Каталог → Свои источники». Сразу ниже —
// персонажи этого источника, которых ещё нет: лицо, «Послушать» (превью голоса из каталога) и «Поставить» — мастер
// установки п. 40 (модель на Турбо, скачивание с ходом, голос). Поставленные становятся обычными карточками шага.
// Решение и ход добавления — в invite-state.ts, одно на все первые шаги (п. 64); здесь — только как это выглядит.
// Строку уберём, когда перестанем раздавать ссылку, — всё здесь и там, на шаге одна строка `<Invite …/>`.

import { useEffect, useRef, useState } from 'preact/hooks';

import { CatalogEntry, CatalogView, api, can, installApi, sessionStore } from '../../api';
import { useAudio } from '../../audio';
import { tr } from '../../i18n';
import { installProgress, onInstallProgress, openInstallWizard } from '../../magicplace/wizard-state';
import { choosePending, dropPending, retryPending, usePendingInstall } from '../../magicplace/pending-install';
import { wordOfEntry } from '../../magicplace/filters';
import { acceptInvite, askInvite, cancelInvite, inviteProblem, setDraft, skipInvite, useInvite } from './invite-state';
import { useStepNav } from './nav';
import { useMediaUrl } from '../../ui/media';

/** Звук приглашения — под ключом шага: уходя со шага, его звук останавливается (character.tsx). */
const SOUND = 'onb-character:invite:';
/** Центр молчит, пока добавляем: повтор через столько. */
const RETRY_MS = 4000;

/** Своих источников у шлюза — не больше (gateway plugins/catalog.rs MAX_SOURCES). */
const MAX_SOURCES = 4;

/** Ссылка из приглашения — адрес ленты: без `catalog.json` на конце он дописывается. null — это не ссылка. */
export function inviteFeed(text: string): string | null {
    let url: URL;
    try {
        url = new URL(text.trim());
    } catch {
        return null;
    }
    if (url.protocol !== 'https:' && url.protocol !== 'http:') return null;
    if (!/\.json$/i.test(url.pathname)) url.pathname = `${url.pathname.replace(/\/+$/, '')}/catalog.json`;
    return url.toString();
}

/** Персонажи своих источников, которых здесь ещё нет; скрытые лентой не показываются (скрытых шлюз и так не присылает). */
function demosOf(view: CatalogView | null): CatalogEntry[] {
    return (view?.packages ?? []).filter((entry) => entry.own && entry.kind === 'character' && entry.state === 'available' && !entry.unlisted);
}

/** Сколько места у центра под пакеты (МБ): свободно на диске минус запас. Не знаем — null. */
function roomMb(free?: number | null, reserve?: number): number | null {
    return typeof free === 'number' ? Math.max(0, free - (reserve ?? 0)) : null;
}

function Face({ src }: { src?: string | null }) {
    const url = useMediaUrl(src);
    return url ? <img class="onb-ch-img" src={url} alt="" /> : null;
}

/**
 * «Есть приглашение?» и персонажи из приглашения. `station` — ключ колонки у компьютера-помощника: источник, каталог и
 * установка — у центра дома через проход; без него — у своего шлюза. `onInstalled` — шаг перечитывает своих персонажей.
 * `defer` — путь с колонкой, а она ещё не центр (настраивается, п. 63): ленту читает этот компьютер, «Поставить» только
 * запоминает выбор, поставит его мастер у центра, как тот будет готов (magicplace/pending-install.tsx). `centreProblem` —
 * перенос не вышел: выбор остаётся, словами сказано, что ждёт.
 */
export function Invite({
    station,
    onInstalled,
    defer = false,
    centreProblem = false,
}: {
    station?: string;
    onInstalled: () => void;
    defer?: boolean;
    centreProblem?: boolean;
}) {
    const t = tr.onbCharacter;
    const at = installApi(station);
    const audio = useAudio();
    // Решение и ход — общие для всех первых шагов (п. 64): уход со шага, перезапуск шлюза и смена дома их не теряют.
    const invite = useInvite();
    const stage = invite.stage;
    const [view, setView] = useState<CatalogView | null>(null);
    const [room, setRoom] = useState<number | null>(null);
    /** Свёрнутая строка «Есть приглашение?» раскрыта. */
    const [open, setOpen] = useState(false);
    /** «Дальше» нажали, не решив: карточка просит решить (п. 53). */
    const [nudged, setNudged] = useState(false);
    const card = useRef<HTMLDivElement | null>(null);
    const input = useRef<HTMLInputElement | null>(null);
    const alive = useRef(true);
    // Свои источники добавляет только администратор у себя дома (gateway plugins/routes.rs admin).
    const mayAdd = can('plugins.manage') && sessionStore.get()?.role === 'admin';

    /** Центр не ответил (он ещё поднимается после переноса): через несколько секунд — ещё раз. */
    const [away, setAway] = useState(false);
    const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
    // Каталог и картинки — из одного места (п. 44): чужой каталог с адресами картинок другого шлюза не показываем.
    const read = async (): Promise<CatalogView | null> => {
        const [catalog, resources] = await Promise.all([at.packagesCatalog().catch(() => null), at.packagesResources().catch(() => null)]);
        if (!alive.current) return null;
        setView(catalog);
        setAway(!catalog && !!station);
        setRoom(resources ? roomMb(resources.disk.freeMb, resources.disk.reserveMb) : null);
        if (timer.current) clearTimeout(timer.current);
        if (!catalog && station) timer.current = setTimeout(() => void read(), RETRY_MS);
        return catalog;
    };
    useEffect(() => {
        alive.current = true;
        void read();
        return () => {
            alive.current = false;
            if (timer.current) clearTimeout(timer.current);
        };
    }, [station]);
    // Набор прочитан (здесь или у нового дома) — перечитать каталог: персонажи видны сразу, без «Дальше» и «Назад».
    useEffect(() => {
        if (stage === 'shown') void read();
    }, [stage, invite.read]);

    const own = (view?.feeds ?? []).filter((feed) => feed.own);

    const check = (): void => {
        const url = inviteFeed(invite.draft);
        if (!url) return inviteProblem(t.inviteNotLink);
        // Уже добавлен здесь — без вопроса: дочитаем и покажем.
        if (own.some((feed) => feed.url === url)) return acceptInvite(url);
        if (own.length >= MAX_SOURCES) return inviteProblem(t.inviteFull);
        askInvite(url);
    };

    const skip = (): void => {
        skipInvite();
        setNudged(false);
        setOpen(false);
    };

    const waiting = usePendingInstall();
    /** Что ставит мастер сейчас: его «Поставить» тут не нужна — шаг показывает его карточкой хода (character.tsx). */
    const [installingId, setInstallingId] = useState<string | undefined>(() => installProgress()?.id);
    useEffect(() => onInstallProgress(() => setInstallingId(installProgress()?.id)), []);
    /** Лента, откуда пакет: свой источник с тем же названием; он один — он; нет — принятая ссылка. */
    const feedOf = (entry: CatalogEntry): string | null =>
        own.find((feed) => feed.title === entry.source)?.url ?? (own.length === 1 ? own[0].url : (invite.url ?? null));

    const install = async (entry: CatalogEntry): Promise<void> => {
        if (defer) {
            const url = feedOf(entry);
            if (!url) return inviteProblem(t.pendingNoFeed);
            choosePending(entry, url, entry.preview?.image ? at.mediaPath(entry.preview.image) : null);
            return;
        }
        const changed = (): void => {
            void read();
            onInstalled();
        };
        const done = await openInstallWizard(entry, changed, false, station);
        if (done !== null) changed();
    };

    const listen = (entry: CatalogEntry): void => {
        const sample = entry.preview?.samples?.[0];
        if (!sample) return;
        void audio.toggle(SOUND + entry.id, () => api.media(at.mediaPath(sample)));
    };

    // Выбор, что ждёт центр, — своей карточкой; центр готов — персонаж ставится сам, его «Поставить» тут не нужна.
    const demos = demosOf(view).filter((entry) => entry.id !== installingId && (defer || entry.id !== waiting?.id));
    const waitingNote = !waiting
        ? ''
        : waiting.problem
          ? waiting.problem
          : waiting.step
            ? t.pendingPreparing
            : centreProblem
              ? t.pendingMoveFailed
              : t.pendingCard;
    // Ссылка принята и в работе: добавляю / читаю — «Дальше» крутится (п. 60); дом молчит — ссылка принята, догрузим
    // сами, «Дальше» свободна.
    const loading = stage === 'accepted' || stage === 'adding' || stage === 'reading';
    const working = loading || stage === 'waiting';
    // Не решено (нет ссылки и не «Пропустить»): заметная карточка, и «Дальше» ведёт к ней же (п. 53). Набор уже есть у
    // дома (добавляли раньше) — решено и так.
    const prominent = mayAdd && stage === 'none' && (away || (view !== null && own.length === 0));
    useStepNav(
        loading
            ? { busy: true }
            : prominent
            ? {
                  onNext: () => {
                      setNudged(true);
                      card.current?.scrollIntoView({ block: 'center', behavior: 'smooth' });
                      input.current?.focus();
                      return false;
                  },
              }
            : {},
        [prominent, loading],
    );
    if (!mayAdd && demos.length === 0 && !waiting) return null;

    // Отдельный блок над карточками шага (п. 58): заголовок, пара строк, что это, поле и кнопки; потом — персонажи
    // набора. Пропустили и набора нет — только строка «Есть приглашение?».
    const block = prominent || working || stage === 'failed' || demos.length > 0 || !!waiting;
    const shown = waiting && !demos.some((entry) => entry.id === waiting.id);
    /** Поле ссылки: решения нет, не вышло (ссылка в поле) или раскрыли «Есть приглашение?». */
    const field = mayAdd && !invite.asking && !working && (prominent || stage === 'failed' || open);
    return (
        <div ref={card} class={`onb-inv${block ? ' onb-inv-block' : ''}${nudged ? ' is-nudged' : ''}`}>
            {block && (
                <div class="onb-inv-block-head">
                    <h2 class="onb-inv-block-title">{t.inviteCardTitle}</h2>
                    <p class={`small onb-ch-p${nudged ? ' onb-ch-warn' : ''}`}>
                        {nudged ? t.inviteChooseFirst : demos.length > 0 ? (defer ? t.inviteBlockDemosLater : t.inviteBlockDemos) : t.inviteCardLead}
                    </p>
                </div>
            )}
            {(demos.length > 0 || shown) && (
                <div class="onb-ch-grid" aria-label={t.inviteDemos}>
                    {shown && waiting && (
                        // Выбор, а его набора здесь нет (центр уже свой, набор ещё читается там): карточка из запомненного.
                        <div class="pcard onb-inv-card is-busy">
                            <div class="pcard-pick">
                                <span class={`face face-cool${waiting.image ? ' onb-ch-has-img' : ''}`}>
                                    <Face src={waiting.image} />
                                </span>
                                <span class="pcard-name">
                                    <span class="onb-inv-name" title={waiting.title}>
                                        {waiting.title}
                                    </span>
                                    {waiting.step && <span class="mark mark-busy onb-inv-spin" aria-hidden="true" />}
                                </span>
                                <span class={`pcard-about${waiting.problem ? ' onb-ch-bad' : ''}`}>{waitingNote}</span>
                            </div>
                            <div class="onb-inv-actions">
                                {waiting.problem && (
                                    <button type="button" class="dw-btn dw-btn-sm dw-btn-dark" onClick={retryPending}>
                                        {t.pendingRetry}
                                    </button>
                                )}
                                <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={dropPending}>
                                    {t.pendingDrop}
                                </button>
                            </div>
                        </div>
                    )}
                    {demos.map((entry) => {
                        const sample = entry.preview?.samples?.[0];
                        const sounding = audio.playing === SOUND + entry.id;
                        // Место — у центра; пока центр не готов, считать по этому компьютеру нечего.
                        const tight = !defer && room !== null && !!entry.sizeMb && entry.sizeMb > room;
                        const chosen = waiting?.id === entry.id;
                        // Своё слово персонажа (п. 65): ставится вместе с ним — видно на карточке.
                        const word = wordOfEntry(entry, view?.packages ?? []);
                        return (
                            <div key={entry.id} class={`pcard onb-inv-card${chosen ? ' is-chosen' : ''}`}>
                                <button
                                    type="button"
                                    class="pcard-pick"
                                    disabled={tight || chosen}
                                    aria-label={t.inviteInstall(entry.title)}
                                    onClick={() => void install(entry)}
                                >
                                    <span class={`face face-cool${entry.preview?.image ? ' onb-ch-has-img' : ''}`}>
                                        <Face src={entry.preview?.image ? at.mediaPath(entry.preview.image) : null} />
                                        <span class="face-line">{entry.description}</span>
                                    </span>
                                    <span class="pcard-name">
                                        <span class="onb-inv-name" title={entry.title}>
                                            {entry.title}
                                        </span>
                                        <span class="onb-inv-chip">{t.inviteChip}</span>
                                    </span>
                                    <span class={`pcard-about${chosen && waiting?.problem ? ' onb-ch-bad' : ''}`}>
                                        {chosen
                                            ? waitingNote
                                            : tight
                                              ? t.inviteNoRoom(entry.sizeMb ?? 0, room ?? 0)
                                              : [entry.sizeMb ? t.inviteSize(entry.sizeMb) : entry.description, word && t.inviteWord(word)]
                                                    .filter(Boolean)
                                                    .join(' · ')}
                                    </span>
                                </button>
                                <div class="onb-inv-actions">
                                    {sample && (
                                        // Кнопкой-значком: «Послушать» и «Поставить» — в один ряд у низа карточки, у всех ровно (п. 57).
                                        <button
                                            type="button"
                                            class="listen onb-inv-listen"
                                            aria-pressed={sounding}
                                            aria-label={sounding ? t.stop : t.listenTo(entry.title)}
                                            title={sounding ? t.stop : t.listen}
                                            onClick={() => listen(entry)}
                                        >
                                            {sounding ? (
                                                <span class="wave" aria-hidden="true">
                                                    <i />
                                                    <i />
                                                    <i />
                                                    <i />
                                                </span>
                                            ) : (
                                                <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                                    <path d="M8 5v14l11-7z" />
                                                </svg>
                                            )}
                                        </button>
                                    )}
                                    {chosen ? (
                                        <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={dropPending}>
                                            {t.pendingDrop}
                                        </button>
                                    ) : (
                                        <button type="button" class="dw-btn dw-btn-sm dw-btn-dark" disabled={tight} onClick={() => void install(entry)}>
                                            {t.inviteInstallShort}
                                        </button>
                                    )}
                                </div>
                            </div>
                        );
                    })}
                </div>
            )}

            {away && (
                <p class="small onb-ch-p" role="status">
                    {t.inviteAway}
                </p>
            )}
            {mayAdd && invite.asking && (
                <div class="onb-inv-row onb-inv-warn" role="alertdialog" aria-label={t.inviteWarnTitle}>
                    <span class="small">
                        <b>{t.inviteWarnTitle}</b> {t.inviteWarn}
                    </span>
                    <span class="onb-inv-buttons">
                        <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={() => askInvite(null)}>
                            {t.inviteCancel}
                        </button>
                        <button type="button" class="dw-btn dw-btn-sm dw-btn-dark" onClick={() => acceptInvite(invite.asking as string)}>
                            {t.inviteAgree}
                        </button>
                    </span>
                </div>
            )}
            {mayAdd && working && (
                // Ссылку приняли: видно, что идёт, — и после ухода со шага и возврата (п. 64).
                <div class="onb-inv-row onb-inv-warn" role="status">
                    <span class="small">
                        <span class="mark mark-busy onb-inv-spin" aria-hidden="true" />
                        {stage === 'waiting' ? t.inviteWaiting : stage === 'reading' ? t.inviteReading : t.inviteAddingSet}
                    </span>
                    <span class="onb-inv-buttons">
                        <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={cancelInvite}>
                            {t.inviteCancel}
                        </button>
                    </span>
                </div>
            )}
            {field && (
                <div class={prominent || stage === 'failed' ? 'onb-inv-card-ask' : undefined}>
                    <form
                        class="onb-inv-row"
                        onSubmit={(e) => {
                            e.preventDefault();
                            check();
                        }}
                    >
                        <input
                            ref={input}
                            class="input"
                            type="url"
                            inputMode="url"
                            autocomplete="off"
                            aria-label={t.inviteLabel}
                            placeholder={t.invitePlaceholder}
                            value={invite.draft}
                            onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
                        />
                        <span class="onb-inv-buttons">
                            {prominent || stage === 'failed' ? (
                                <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={skip}>
                                    {t.inviteSkip}
                                </button>
                            ) : (
                                <button type="button" class="dw-btn dw-btn-sm dw-btn-light" onClick={() => (setOpen(false), inviteProblem(undefined))}>
                                    {t.inviteCancel}
                                </button>
                            )}
                            <button type="submit" class="dw-btn dw-btn-sm dw-btn-dark" disabled={!invite.draft.trim()}>
                                {t.inviteAdd}
                            </button>
                        </span>
                    </form>
                </div>
            )}
            {mayAdd && !field && !working && !invite.asking && (
                <button type="button" class="onb-inv-ask small" onClick={() => setOpen(true)}>
                    {t.inviteAsk}
                </button>
            )}
            {invite.problem && (
                <p class="small onb-ch-p onb-ch-bad" role="alert">
                    {invite.problem}
                </p>
            )}
            {stage === 'shown' && !invite.problem && !open && (
                <p class="small onb-ch-p" role="status">
                    {demos.length ? t.inviteAdded(demos.length) : t.inviteEmpty}
                </p>
            )}
        </div>
    );
}
