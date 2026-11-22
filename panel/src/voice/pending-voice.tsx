// «Голос ждёт» с любого экрана (проверка начисто 19.11, первый абзац Bugs.md). Голос, который ждёт решения владельца —
// «Всё встало, но ОЗУ впритык… Включить всё равно?», — жил только на странице компьютера («Настройки» → «Устройства» →
// Mac → «Этот компьютер»): пока персонаж донастраивался, человек этого вопроса не видел, и голос висел. Теперь ждущий
// голос дома читается у центра (`/api/v1/home/voice/pending`: свой или Турбо, что даёт голос), и его видно везде:
// пилюля у дока «… ждёт вашего решения», точка на «Настройках» и на «Голосе», карточка вверху «Голоса», а окно с
// вопросом открывается само — один раз на каждое ожидание. Голос, что просто ставится (ответа не ждёт), тоже виден
// (21.11): спокойная пилюля у дока с моделью и процентом — без точки и без окна, что открывается само.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, HomeVoiceAnswer, HomeVoiceWaiting, TtsPending, api, can, ownCall, problemOf } from '../api';
import { tr } from '../i18n';
import { Action } from '../ui/cards';
import { Progress, needsWhat } from '../ui/needs';
import { Sheet } from '../ui/sheet';
import { VIA } from '../via';

/** Как часто спрашивать центр. Ответ — ждущий голос из настроек и ход задания: дёшево и для центра, и для Турбо. */
const READ_EVERY_MS = 15_000;
/** Ожидания, чьё окно уже открывалось само: второй раз не навязываем, пилюля и точка остаются. */
const SEEN_KEY = 'voice-waits-seen';

let now: HomeVoiceWaiting | null = null;
let open = false;
/** Центр старый (404): маршрута нет — его не спрашиваем. */
let older = false;
/** Где ждёт голос, о котором сказано: у этого компьютера (окно колонки на Турбо) или у центра. */
let from: 'own' | 'centre' = 'centre';

const PATH = '/api/v1/home/voice/pending';
const NOTHING: HomeVoiceWaiting = { turbo: null, pending: null };

/**
 * Что ждёт. В окне колонки (`?via=`) этот компьютер и есть Турбо, что даёт ей голос: сначала спрашиваем его самого —
 * так вопрос виден и при центре постарше, у которого маршрута ещё нет; у него не ждёт ничего — спрашиваем центр.
 */
async function read(): Promise<HomeVoiceWaiting> {
    const own = VIA ? await ownCall<HomeVoiceWaiting>('GET', PATH).catch(() => null) : null;
    if (own?.pending) {
        from = 'own';
        return own;
    }
    from = 'centre';
    if (older) return own ?? NOTHING;
    try {
        return await api.homeVoiceWaiting();
    } catch (e) {
        if (e instanceof ApiError && (e.status === 404 || e.status === 501)) {
            older = true;
            return own ?? NOTHING;
        }
        throw e;
    }
}

/** Ответ туда, где голос ждёт. */
const send = (answer: HomeVoiceAnswer): Promise<HomeVoiceWaiting> =>
    from === 'own' ? ownCall<HomeVoiceWaiting>('POST', PATH, { answer }) : api.answerHomeVoiceWaiting(answer);
let readNow: (() => void) | null = null;
const listeners = new Set<() => void>();

function notify(): void {
    for (const listener of [...listeners]) listener();
}

type Waiting = NonNullable<HomeVoiceWaiting['pending']>;

/** Ждёт ответа владельца: памяти впритык или то, чего голос ждал, не вышло. Ставится или качается — не ждёт. */
export const asksOwner = (pending: TtsPending | null | undefined): boolean => pending?.state === 'tight' || pending?.state === 'failed';

/** Имя без пометки: «Паша Техник (демо)» → «Паша Техник». */
export const plainName = (title: string): string => title.replace(/\s*\([^)]*\)\s*$/, '').trim() || title;

/** Ставится или качается, ответа не ждёт: спокойная пилюля у дока, без точки и без окна, что открывается само. */
export const goesOn = (pending: TtsPending | null | undefined): boolean => !!pending && !asksOwner(pending);

/** Имя модели, которую ждёт голос, для людей («CosyVoice 3»); шлюз постарше его не присылает — null. */
const modelTitle = (pending: TtsPending): string | null => (pending.waitFor?.kind === 'engine' ? (pending.waitFor.title ?? pending.needs?.title ?? null) : null);

/** Что с голосом, словами — те же, что у карточки «Голос ждёт» на странице компьютера (screens/models.tsx). */
export function pendingWords(pending: TtsPending, title: string): string {
    const kind = pending.waitFor?.kind ?? pending.needs?.kind ?? 'engine';
    const model = modelTitle(pending);
    // Модель — по имени: «Ставлю модель CosyVoice 3 — …», а не безымянную «модель».
    const what = pending.needs
        ? needsWhat(pending.needs)
        : kind === 'file'
          ? tr.models.whatVoice
          : kind === 'imprint'
            ? tr.models.whatImprint
            : model
              ? tr.ui.needsModel(model)
              : tr.models.whatModel;
    if (pending.state === 'tight') return tr.models.pendingTight(title, pending.reason ?? '');
    if (pending.state === 'failed')
        return tr.models.pendingFailed(kind === 'file' ? tr.models.verbDownload : kind === 'imprint' ? tr.models.verbImprint : tr.models.verbInstall, what, pending.reason ?? '');
    return kind === 'file' ? tr.models.pendingDownloading(what, title) : tr.models.pendingInstalling(what, title);
}

function seenKey(pending: Waiting): string {
    return `${pending.modelId}|${pending.since ?? ''}|${pending.state}`;
}

/** Окно этого ожидания уже открывалось само; отмечает его открытым. */
function firstTime(pending: Waiting): boolean {
    const key = seenKey(pending);
    try {
        const seen: string[] = JSON.parse(localStorage.getItem(SEEN_KEY) ?? '[]');
        if (seen.includes(key)) return false;
        localStorage.setItem(SEEN_KEY, JSON.stringify([...seen.slice(-9), key]));
    } catch {
        // приватное окно: откроется один раз за жизнь вкладки
    }
    return true;
}

function take(next: HomeVoiceWaiting | null): void {
    now = next;
    const pending = next?.pending;
    if (!pending) open = false;
    else if (asksOwner(pending) && can('models.manage') && firstTime(pending)) open = true;
    notify();
}

function useWaitingState(): { now: HomeVoiceWaiting | null; open: boolean } {
    const [, rerender] = useState(0);
    useEffect(() => {
        const listener = () => rerender((n) => n + 1);
        listeners.add(listener);
        return () => {
            listeners.delete(listener);
        };
    }, []);
    return { now, open };
}

/** Ждущий голос дома, как его знает центр; null — ничего не ждёт или ещё не прочитано. */
export function useVoiceWaiting(): HomeVoiceWaiting | null {
    return useWaitingState().now;
}

/** Голос ждёт ответа владельца — точка на «Настройках» и на «Голосе». */
export function useVoiceAttention(): boolean {
    return asksOwner(useWaitingState().now?.pending);
}

/** Голос ставится или качается и ответа не ждёт — спокойная пилюля у дока (без точки). */
export function useVoiceGoing(): boolean {
    return goesOn(useWaitingState().now?.pending);
}

export function showVoiceWaiting(): void {
    open = true;
    notify();
}

function closeVoiceWaiting(): void {
    open = false;
    notify();
}

/** Перечитать сейчас: после своего ответа или когда страница «Голос» открылась. */
export function rereadVoiceWaiting(): void {
    readNow?.();
}

/** Спрашивает центр, пока пульт открыт; стоит рядом с мастером установки (app.tsx) — на любом экране. */
export function VoiceWaitingRunner() {
    const { now: shown, open: opened } = useWaitingState();
    useEffect(() => {
        if (!can('station.view')) return;
        let alive = true;
        let timer: ReturnType<typeof setTimeout> | undefined;
        let busy = false;
        const look = async (): Promise<void> => {
            if (busy) return;
            busy = true;
            if (timer) clearTimeout(timer);
            try {
                const next = await read();
                if (alive) take(next);
            } catch {
                // не ответил — спросим в следующий раз
            } finally {
                busy = false;
            }
            // Центр старый и спросить больше некого — не спрашиваем.
            if (alive && (!older || VIA)) timer = setTimeout(() => void look(), READ_EVERY_MS);
        };
        readNow = () => void look();
        void look();
        return () => {
            alive = false;
            readNow = null;
            if (timer) clearTimeout(timer);
        };
    }, []);
    return opened && shown?.pending ? <WaitingSheet waiting={shown} pending={shown.pending} /> : null;
}

/** Ответить ждущему голосу: там, где он ждёт; ответ шлюза — что ждёт теперь. */
function useAnswer(): { busy: HomeVoiceAnswer | null; error: string | null; answer: (what: HomeVoiceAnswer) => Promise<boolean> } {
    const [busy, setBusy] = useState<HomeVoiceAnswer | null>(null);
    const [error, setError] = useState<string | null>(null);
    const answer = async (what: HomeVoiceAnswer): Promise<boolean> => {
        setBusy(what);
        setError(null);
        try {
            take(await send(what));
            return true;
        } catch (e) {
            setError(problemOf(e));
            rereadVoiceWaiting();
            return false;
        } finally {
            setBusy(null);
        }
    };
    return { busy, error, answer };
}

/**
 * Запасной голос персонажа (замечание 14 после 0.18.1): не «Дмитрий ставится», а чей он и зачем — «Голос персонажа
 * «Рик» на этом компьютере не запустится. На Windows нужна видеокарта NVIDIA. Ставлю запасной голос «Дмитрий (Piper)».
 * Здесь «Рик» будет говорить им».
 */
function BackupWhy({ pending }: { pending: Waiting }) {
    const backup = pending.backupOf;
    if (!backup) return null;
    const who = tr.wizard.quote(plainName(backup.title));
    return (
        <p class="m-0 t-body-sm">
            {tr.voiceHome.backupCant(who)}
            {backup.why ? ` ${backup.why}` : ''} {tr.voiceHome.backupInstall(who, tr.models.quoted(pending.title))}
        </p>
    );
}

/** Подпись окна и карточки: имя голоса, у запасного — персонаж («Рик» · запасной голос). */
const waitingChip = (pending: Waiting): string =>
    pending.backupOf ? tr.voiceHome.backupChip(tr.wizard.quote(plainName(pending.backupOf.title))) : tr.models.quoted(pending.title);

/** Текст, ход и подробность ждущего голоса. */
function WaitingText({ waiting, pending }: { waiting: HomeVoiceWaiting; pending: Waiting }) {
    const going = !asksOwner(pending);
    return (
        <>
            <BackupWhy pending={pending} />
            {/* Запасной ставится — его слова выше; ждёт ответа (память, не вышло) — и слова шлюза об этом. */}
            {!(pending.backupOf && going) && (
                <p
                    class={`m-0 t-body-sm${pending.state === 'tight' ? ' c-pill-yandex-fg' : pending.state === 'failed' ? ' c-dot-offline' : ''}`}
                    role={pending.state === 'failed' ? 'alert' : 'status'}
                >
                    {pendingWords(pending, pending.title)}
                </p>
            )}
            {going && pending.progress !== undefined && pending.progress !== null && <Progress percent={pending.progress} label={tr.models.installProgress(pending.title)} />}
            {pending.detail && <span class="t-caption">{pending.detail}</span>}
            {asksOwner(pending) && <span class="t-caption">{tr.voiceHome.waitMeanwhile}</span>}
            {waiting.turbo && !waiting.turbo.up && <span class="t-caption">{tr.models.computerDown}</span>}
        </>
    );
}

/** Окно с вопросом — открывается само один раз на ожидание, потом — с пилюли у дока. */
function WaitingSheet({ waiting, pending }: { waiting: HomeVoiceWaiting; pending: Waiting }) {
    const { busy, error, answer } = useAnswer();
    const admin = can('models.manage');
    const done = (ok: boolean): void => {
        if (ok) closeVoiceWaiting();
    };
    return (
        <Sheet
            title={tr.voiceHome.waitTitle}
            meta={<span class="win-chip">{waitingChip(pending)}</span>}
            about={waiting.turbo ? tr.voiceHome.waitOn(waiting.turbo.name) : undefined}
            onClose={closeVoiceWaiting}
            footer={
                <>
                    <button class="btn btn-quiet" onClick={closeVoiceWaiting}>
                        {asksOwner(pending) ? tr.voiceHome.waitLater : tr.wizard.close}
                    </button>
                    {admin && (
                        <button class="btn btn-plain" disabled={!!busy} aria-busy={busy === 'cancel' || undefined} onClick={() => void answer('cancel').then(done)}>
                            {tr.models.cancelUpper}
                        </button>
                    )}
                    {admin && pending.state === 'tight' && (
                        <button class="btn btn-main" disabled={!!busy} aria-busy={busy === 'anyway' || undefined} onClick={() => void answer('anyway').then(done)}>
                            {busy === 'anyway' && <span class="spinner spinner-sm" aria-hidden="true" />}
                            {tr.models.enableAnyway}
                        </button>
                    )}
                    {admin && pending.state === 'failed' && (
                        <button class="btn btn-main" disabled={!!busy} aria-busy={busy === 'retry' || undefined} onClick={() => void answer('retry').then(done)}>
                            {busy === 'retry' && <span class="spinner spinner-sm" aria-hidden="true" />}
                            {tr.models.tryAgain}
                        </button>
                    )}
                </>
            }
        >
            <div class="flex flex-col gap-8">
                <WaitingText waiting={waiting} pending={pending} />
                {!admin && <span class="t-caption">{tr.voiceHome.waitAdminOnly}</span>}
                {error && (
                    <p class="win-callout m-0" role="alert">
                        {error}
                    </p>
                )}
            </div>
        </Sheet>
    );
}

/** Карточка вверху «Голоса»: тот же вопрос, что в окне, и всё, что ставится, — пока голос ждёт. */
export function WaitingCard() {
    const waiting = useVoiceWaiting();
    const { busy, error, answer } = useAnswer();
    useEffect(() => rereadVoiceWaiting(), []);
    const pending = waiting?.pending;
    if (!waiting || !pending) return null;
    const admin = can('models.manage');
    return (
        <section class="card glass card-list" aria-labelledby="voice-wait-h">
            <div class="card-head">
                <h2 id="voice-wait-h" class="t-title-xs m-0">
                    {tr.voiceHome.waitTitle}
                </h2>
                <span class="t-meta">{waitingChip(pending)}</span>
            </div>
            <div class="pending-row">
                <WaitingText waiting={waiting} pending={pending} />
                {error && (
                    <span class="t-caption c-dot-offline" role="alert">
                        {error}
                    </span>
                )}
                {admin && (
                    <div class="flex flex-wrap items-center gap-8">
                        {pending.state === 'tight' && <Action tone="on" label={tr.models.enableAnyway} busy={busy === 'anyway'} disabled={!!busy} onClick={() => void answer('anyway')} />}
                        {pending.state === 'failed' && <Action tone="on" label={tr.models.tryAgain} busy={busy === 'retry'} disabled={!!busy} onClick={() => void answer('retry')} />}
                        <Action label={tr.models.cancelUpper} busy={busy === 'cancel'} disabled={!!busy} onClick={() => void answer('cancel')} />
                    </div>
                )}
            </div>
        </section>
    );
}

/** Спокойная строка пилюли, пока голос ставится: «Рик: ставлю CosyVoice 3 · 37 %». */
function goingLine(pending: Waiting, name: string): string {
    const t = tr.voiceHome;
    const kind = pending.waitFor?.kind ?? 'engine';
    const model = modelTitle(pending);
    const line = pending.backupOf
        ? t.waitPillBackup(tr.wizard.quote(plainName(pending.backupOf.title)))
        : kind === 'file'
          ? t.waitPillFile(name)
          : kind === 'imprint'
            ? t.waitPillImprint(name)
            : model
              ? t.waitPillModel(name, model)
              : t.waitPillAnyModel(name);
    return pending.progress !== undefined && pending.progress !== null ? t.waitPillPercent(line, Math.round(pending.progress)) : line;
}

/**
 * Пилюля у дока: голос ждёт ответа — «Паша Техник ждёт вашего решения», с точкой; голос просто ставится — спокойно,
 * «Рик: ставлю CosyVoice 3 · 37 %» (скачивание голоса не пряталось бы — 21.11). Нажатие — окно голоса.
 */
export function VoiceWaitingPill({ place }: { place: 'top' | 'side' | 'onb' }) {
    const waiting = useVoiceWaiting();
    const pending = waiting?.pending;
    if (!pending) return null;
    // Запасной голос ждёт — пилюля о персонаже, которому он нужен: «Рик ждёт вашего решения», а не «Дмитрий».
    const name = tr.wizard.quote(plainName(pending.backupOf?.title ?? pending.title));
    if (!asksOwner(pending))
        return (
            <button type="button" class={`waiting-pill waiting-pill-${place}`} onClick={showVoiceWaiting}>
                {goingLine(pending, name)}
            </button>
        );
    return (
        <button type="button" class={`waiting-pill waiting-pill-${place} is-attention`} onClick={showVoiceWaiting}>
            <span class="badge-dot" aria-hidden="true" />
            {pending.state === 'failed' ? tr.voiceHome.waitPillFailed(name) : tr.voiceHome.waitPill(name)}
        </button>
    );
}
