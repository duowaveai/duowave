// «Поделиться» и «Записать своим голосом» — окна пульта с любого экрана (docs/plans/feed.md, этап 5; слой возможностей
// шлюза, gateway/src/capabilities). Окошко приложения само наружу не ходит и микрофона не получает: оно просит пульт,
// а пульт показывает своё окно — что уходит, куда, и кнопки. Экран пульта («Папки») открывает те же окна для файла
// папки дома по адресу `files/…`. Отправку начинает человек своим нажатием — отдельного «да» не нужно; шлюз помнит в
// журнале отправителя (приложения или шины от имени человека), что и куда ушло. Куда можно отправить — список шлюза
// (`/api/v1/publishers`), не окошка: приложение не может подсунуть свою ссылку.

import { useEffect, useRef, useState } from 'preact/hooks';

import { PublisherCard, api, can, problemOf, uploadAppRecording, uploadFolderRecording } from '../api';
import { tr } from '../i18n';
import { rightWords as busRightWords } from '../bus/rights';
import { rightWords } from '../plugins/rights';
import { Recorder, recordingSupport } from '../recorder';
import { Sheet, WinRow, WinSection } from '../ui/sheet';
import { mediaUrl } from './media';

/** Что окошко отдаёт наружу: файл своей папки и/или текст к посту. */
export interface ShareRequest {
    file?: string;
    text?: string;
    title?: string;
}

/** Куда ушло — или null, если человек закрыл окно. */
export interface ShareResult {
    where: string | null;
    saved?: string | null;
}

interface PendingShare {
    /** Приложение, из чьей папки файл; null — человек делится файлом папки дома (`request.file` — адрес `files/…`). */
    app: string | null;
    request: ShareRequest;
    /** Где браузер возьмёт файл папки дома (просмотрщик «Папок»). */
    url?: string;
    done: (result: ShareResult) => void;
}

interface PendingRecord {
    /** Куда ложится запись: папка приложения или папка дома (`files/…`). */
    into: { app: string } | { folder: string };
    options: RecordOptions;
    done: (result: { file: string; seconds: number } | null, error?: string) => void;
}

export interface RecordOptions {
    title?: string;
    /** Что читать: строка реплики. */
    text?: string;
    maxSeconds?: number;
}

/** Связь, что приложение просит в манифесте: куда, каким уровнем, словами, разрешена ли уже. */
export interface AppRight {
    right: string;
    to: string;
    level: string;
    words: string;
    allowed: boolean;
    /** Что-то уходит из дома (облачный голос): на листе — янтарным. */
    out: boolean;
}

export interface AppRights {
    asked: AppRight[];
    pending: number;
}

const LEVELS = ['see', 'listen', 'call', 'edit'];

/** Связи приложения на шине: что его манифест просит и что владелец уже разрешил. */
export async function rightsOfApp(app: string): Promise<AppRights & { title: string }> {
    const [packages, links] = await Promise.all([api.packages(), api.busLinks()]);
    const own = packages.packages.find((entry) => entry.id === app);
    const asked = (own?.permissions ?? [])
        .filter((right) => /^(see|listen|call|edit):/.test(right))
        .map((right) => {
            const at = right.indexOf(':');
            const [level, to] = [right.slice(0, at), right.slice(at + 1)];
            const link = links.links.find((entry) => entry.from === `app/${app}` && entry.to === to);
            const allowed = !!link && LEVELS.indexOf(link.level) >= LEVELS.indexOf(level);
            return { right, to, level, words: rightWords(right), allowed, out: busRightWords(right).out };
        });
    return { asked, pending: asked.filter((right) => !right.allowed).length, title: own?.title ?? app };
}

interface PendingRights {
    app: string;
    title: string;
    rights: AppRight[];
    done: (allowed: boolean) => void;
}

let showRights: ((next: PendingRights | null) => void) | null = null;

/**
 * Все «да» приложения одним листом (знакомство «Ленты»): вместо листа на каждую связь в разгар работы — что оно
 * просит, словами, и «Разрешить всё». Разрешает администратор дома; ответ — связи после этого.
 */
export async function askRightsForApp(app: string): Promise<AppRights> {
    const state = await rightsOfApp(app);
    if (!state.pending) return state;
    if (!can('plugins.manage')) throw new Error(tr.share.rightsAdminOnly);
    if (!showRights) throw new Error(tr.share.cannotAsk);
    const pending = state.asked.filter((right) => !right.allowed);
    const allowed = await new Promise<boolean>((resolve) => showRights?.({ app, title: state.title, rights: pending, done: resolve }));
    if (allowed) for (const right of pending) await api.busLink(`app/${app}`, right.to, right.level as Parameters<typeof api.busLink>[2]);
    return rightsOfApp(app);
}

let showShare: ((next: PendingShare | null) => void) | null = null;
let showRecord: ((next: PendingRecord | null) => void) | null = null;

/** «Поделиться» с любого экрана пульта или из окна приложения. */
export interface ShareFrom {
    /** Файл: `files/…` — папка дома, от лица вошедшего; при `from: 'app/<id>'` — путь в папке приложения. */
    address?: string;
    /** Кто отправляет: `person` — вошедший в пульт (по умолчанию) или `app/<id>` — окно приложения. Журнал — ему. */
    from?: string;
    /**
     * Где браузер возьмёт сам файл — для меню «Поделиться» телефона и «Скачать» (просмотрщик «Папок»). Файл
     * приложения пульт берёт сам; без `url` у файла папки дома остаются отправители шлюза и площадки.
     */
    url?: string;
    /** Текст к посту. */
    text?: string;
    title?: string;
}

/** Запись своим голосом с любого экрана пульта или из окна приложения. */
export interface RecordFor {
    /** Папка дома, куда ляжет запись (`files/…`), — для `person`; у приложения — его папка, `recordings/<время>.wav`. */
    folder?: string;
    /** Кто записывает: `person` — вошедший в пульт (по умолчанию) или `app/<id>` — окно приложения. */
    from?: string;
    title?: string;
    /** Что читать: строка реплики. */
    text?: string;
    /** Длина записи, 3–300 с (по умолчанию 60). */
    maxSeconds?: number;
}

/** Отправитель словами шлюза: приложение по его id или человек (null). */
function senderOf(from: string | undefined): { app: string | null } | Error {
    if (!from || from === 'person') return { app: null };
    const app = /^app\/([a-z0-9][a-z0-9._-]*)$/i.exec(from)?.[1];
    return app ? { app } : new Error(tr.share.badSender);
}

/** Окно «Поделиться»: что уходит, куда (список отправителей и площадок — от шлюза) и кнопки; ответ — куда ушло. */
export function shareFrom(request: ShareFrom): Promise<ShareResult> {
    if (!showShare) return Promise.reject(new Error(tr.share.cannotShare));
    const sender = senderOf(request?.from);
    if (sender instanceof Error) return Promise.reject(sender);
    const file = typeof request?.address === 'string' && request.address ? request.address.slice(0, 300) : undefined;
    if (file && !sender.app && !file.startsWith('files/')) return Promise.reject(new Error(tr.share.fileNotHouse));
    const clean: ShareRequest = {
        file,
        text: typeof request?.text === 'string' ? request.text.slice(0, 4000) : undefined,
        title: typeof request?.title === 'string' ? request.title.slice(0, 120) : undefined,
    };
    if (!clean.file && !clean.text) return Promise.reject(new Error(tr.share.nothingToSend));
    const url = !sender.app && typeof request?.url === 'string' && request.url ? request.url : undefined;
    return new Promise((resolve) => showShare?.({ app: sender.app, request: clean, url, done: resolve }));
}

/** Окно «Поделиться» для окошка приложения `app` (файл его папки) — `shareFrom` от его имени. */
export function shareFromApp(app: string, request: ShareRequest): Promise<ShareResult> {
    return shareFrom({ address: request?.file, from: `app/${app}`, text: request?.text, title: request?.title });
}

/** Окно записи своим голосом: микрофон открывает пульт; ответ — файл записи (адрес `files/…` или путь в папке приложения). */
export function recordFor(request: RecordFor): Promise<{ file: string; seconds: number }> {
    if (!showRecord) return Promise.reject(new Error(tr.share.cannotRecord));
    const sender = senderOf(request?.from);
    if (sender instanceof Error) return Promise.reject(sender);
    const folder = typeof request?.folder === 'string' ? request.folder.trim() : '';
    if (!sender.app && !folder.startsWith('files/')) return Promise.reject(new Error(tr.share.folderNotHouse));
    const into: PendingRecord['into'] = sender.app ? { app: sender.app } : { folder };
    const clean: RecordOptions = {
        title: typeof request?.title === 'string' ? request.title.slice(0, 120) : undefined,
        text: typeof request?.text === 'string' ? request.text.slice(0, 2000) : undefined,
        maxSeconds: Math.max(3, Math.min(300, Number(request?.maxSeconds) || 60)),
    };
    return new Promise((resolve, reject) =>
        showRecord?.({ into, options: clean, done: (result, error) => (result ? resolve(result) : reject(new Error(error ?? tr.share.recordCancelled))) }),
    );
}

/** Окно записи своим голосом для окошка приложения `app` — `recordFor` от его имени; запись — в его папке. */
export function recordForApp(app: string, options: RecordOptions): Promise<{ file: string; seconds: number }> {
    return recordFor({ ...options, from: `app/${app}` });
}

/** Какой это файл для отправителей: видео, картинка, текст — по расширению, как в шлюзе (capabilities/publish.rs). */
function kindOf(path: string): string | null {
    const extension = path.split('.').pop()?.toLowerCase() ?? '';
    if (['mp4', 'mov', 'webm', 'm4v'].includes(extension)) return 'video';
    if (['jpg', 'jpeg', 'png', 'webp', 'gif'].includes(extension)) return 'image';
    if (['txt', 'md'].includes(extension)) return 'text';
    return null;
}

function fileName(path: string): string {
    return path.split('/').filter(Boolean).pop() ?? 'file';
}

function ShareSheet({ pending, close }: { pending: PendingShare; close: (result: ShareResult) => void }) {
    const { app, request, url } = pending;
    const [file, setFile] = useState<File | null>(null);
    // Файл в окно браузера: из папки приложения — сам пульт; файл папки дома — по адресу, что дал экран («Папки»).
    const [preparing, setPreparing] = useState(!!request.file && (!!app || !!url));
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [said, setSaid] = useState<string | null>(null);
    const [publishers, setPublishers] = useState<PublisherCard[] | null>(null);
    const kind = request.file ? kindOf(request.file) : 'text';

    useEffect(() => {
        api.publishers()
            .then((answer) => setPublishers(answer.publishers))
            .catch(() => setPublishers([]));
    }, []);

    // Файл готовим сразу, как открылось окно: системное меню телефона пускают только по свежему нажатию.
    useEffect(() => {
        if (!request.file || (!app && !url)) return;
        let alive = true;
        (app ? mediaUrl(app, request.file) : Promise.resolve(url!))
            .then((source) => fetch(source))
            .then((response) => {
                if (!response.ok) throw new Error(`HTTP ${response.status}`);
                return response.blob();
            })
            .then((blob) => alive && setFile(new File([blob], fileName(request.file!), { type: blob.type || 'application/octet-stream' })))
            .catch((error) => alive && setProblem(tr.share.fileNotReady((error as Error).message)))
            .finally(() => alive && setPreparing(false));
        return () => {
            alive = false;
        };
    }, [app, request.file, url]);

    const note = async (where: string, withFile = true): Promise<string | null> => {
        try {
            const answer = await api.shared({ app: app ?? undefined, where, file: withFile ? request.file : undefined });
            return answer.saved ?? null;
        } catch (error) {
            setProblem(problemOf(error));
            return null;
        }
    };
    const canSystem = !!file && typeof navigator.canShare === 'function' && navigator.canShare({ files: [file] });
    const system = async (): Promise<void> => {
        if (!file) return;
        try {
            await navigator.share({ files: [file], text: request.text, title: request.title });
            await note('system');
            close({ where: 'system' });
        } catch (error) {
            if ((error as DOMException).name !== 'AbortError') setProblem(tr.share.menuFailed((error as Error).message));
        }
    };
    const videos = async (): Promise<void> => {
        setBusy('videos');
        const saved = await note('videos');
        setBusy(null);
        if (saved) setSaid(tr.share.savedToVideos(fileName(saved)));
    };
    const download = async (): Promise<void> => {
        if (!file) return;
        const url = URL.createObjectURL(file);
        const link = document.createElement('a');
        link.href = url;
        link.download = file.name;
        document.body.appendChild(link);
        link.click();
        link.remove();
        setTimeout(() => URL.revokeObjectURL(url), 60_000);
        await note('download');
        setSaid(tr.share.downloaded);
    };
    const copy = async (): Promise<void> => {
        if (!request.text) return;
        try {
            await navigator.clipboard.writeText(request.text);
            await note('copy', false);
            setSaid(tr.share.textCopied);
        } catch {
            setProblem(tr.share.copyRefused);
        }
    };
    const page = async (place: PublisherCard): Promise<void> => {
        if (!place.url) return;
        window.open(place.url, '_blank', 'noopener,noreferrer');
        await note(place.id, !!request.file);
        setSaid(tr.share.pageOpened(place.title, !!app));
    };
    const direct = async (place: PublisherCard): Promise<void> => {
        if (!request.file) return;
        setBusy(place.id);
        setProblem(null);
        try {
            const answer = await api.publishSend(place.id, {
                app: app ?? undefined,
                file: request.file,
                input: request.text ? { caption: request.text.slice(0, 1000) } : {},
            });
            const path = typeof answer?.path === 'string' ? answer.path : null;
            setSaid(path ? `${place.title}: ${location.origin}${path}` : tr.share.sent(place.title));
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(null);
        }
    };

    // Кто возьмёт этот файл: отправители шлюза — что берут, чьи файлы и готовы ли; страницы площадок — что берут.
    const fits = (place: PublisherCard): boolean => !!kind && place.accepts.includes(kind) && (place.from === 'any' || !!app);
    const senders = (publishers ?? []).filter((place) => place.kind === 'direct' && !!request.file && fits(place));
    const pages = (publishers ?? []).filter((place) => place.kind === 'manual' && (!request.file || fits(place)));

    const menuReady = typeof navigator.canShare === 'function';
    const spin = (on: boolean) => on && <span class="spinner spinner-sm" aria-hidden="true" />;

    // Закрыть — крестиком, Esc или мимо: отправили что-то — «done», нет — null (как прежняя «Готово» / «Закрыть»).
    return (
        <Sheet
            title={request.title || tr.share.title}
            about={request.file ? fileName(request.file) : tr.share.textOnly}
            onClose={() => close({ where: said ? 'done' : null })}
            footer={
                canSystem || (preparing && menuReady) ? (
                    <button class="btn btn-main" disabled={!canSystem} aria-busy={preparing || undefined} onClick={() => void system()}>
                        {spin(preparing)}
                        {tr.share.sendMenu}
                    </button>
                ) : undefined
            }
        >
            {problem && (
                <p class="win-callout m-0" role="alert">
                    {problem}
                </p>
            )}
            {said && (
                <p class="win-callout is-calm m-0 wrap-anywhere" role="status">
                    {said}
                </p>
            )}
            {request.text && (
                <WinSection group>
                    <div class="win-row is-col">
                        <p class="win-text m-0 pre-line">{request.text}</p>
                        <button class="btn btn-plain btn-sm self-start" onClick={() => void copy()}>
                            {tr.share.copyText}
                        </button>
                    </div>
                </WinSection>
            )}
            {request.file && (app || url) && (
                <div class="flex flex-wrap gap-8">
                    {app && (
                        <button class="btn btn-plain" disabled={busy === 'videos'} aria-busy={busy === 'videos' || undefined} onClick={() => void videos()}>
                            {spin(busy === 'videos')}
                            {tr.share.saveToVideos}
                        </button>
                    )}
                    <button class="btn btn-plain" disabled={!file} aria-busy={preparing || undefined} onClick={() => void download()}>
                        {spin(preparing)}
                        {tr.share.download}
                    </button>
                </div>
            )}
            {senders.length > 0 && (
                <WinSection title={tr.share.gatewaySendsTitle} group>
                    {senders.map((place) => (
                        <WinRow key={place.id} title={place.title} sub={place.ready ? place.about : (place.needs ?? place.about)}>
                            <button
                                class="btn btn-plain btn-sm"
                                disabled={!place.ready || busy === place.id}
                                aria-busy={busy === place.id || undefined}
                                onClick={() => void direct(place)}
                            >
                                {spin(busy === place.id)}
                                {busy === place.id ? tr.share.sending : tr.share.send}
                            </button>
                        </WinRow>
                    ))}
                </WinSection>
            )}
            {pages.length > 0 && (
                <WinSection title={tr.share.pagesTitle} group>
                    {pages.map((place) => (
                        <WinRow key={place.id} title={place.title} sub={place.about}>
                            <button class="btn btn-plain btn-sm" onClick={() => void page(place)}>
                                {tr.share.openPage}
                            </button>
                        </WinRow>
                    ))}
                </WinSection>
            )}
        </Sheet>
    );
}

function RecordSheet({ pending, close }: { pending: PendingRecord; close: (result: { file: string; seconds: number } | null, error?: string) => void }) {
    const { into, options } = pending;
    const support = recordingSupport();
    const recorder = useRef<Recorder | null>(null);
    const [seconds, setSeconds] = useState(0);
    const [level, setLevel] = useState(-90);
    const [state, setState] = useState<'idle' | 'recording' | 'saving'>('idle');
    const [problem, setProblem] = useState<string | null>(support.ok ? null : support.reason ?? null);
    const max = options.maxSeconds ?? 60;

    useEffect(() => () => recorder.current?.cancel(), []);
    const stop = async (): Promise<void> => {
        const running = recorder.current;
        if (!running) return;
        recorder.current = null;
        setState('saving');
        try {
            const { wav } = await running.stop();
            close(await ('app' in into ? uploadAppRecording(into.app, wav) : uploadFolderRecording(into.folder, wav)));
        } catch (error) {
            setProblem(problemOf(error));
            setState('idle');
        }
    };
    useEffect(() => {
        if (state === 'recording' && seconds >= max) void stop();
    }, [seconds, state]);
    const start = async (): Promise<void> => {
        setProblem(null);
        try {
            recorder.current = await Recorder.start(undefined, (db, at) => {
                setLevel(db);
                setSeconds(at);
            });
            setState('recording');
        } catch (error) {
            setProblem((error as Error).message);
        }
    };
    const width = Math.max(0, Math.min(100, ((level + 60) / 60) * 100));
    const saving = state === 'saving';
    return (
        <Sheet
            title={options.title || tr.share.recordTitle}
            about={tr.share.upTo(max)}
            onClose={() => close(null, tr.share.recordCancelled)}
            footer={
                <>
                    <button class="btn btn-plain" onClick={() => close(null, tr.share.recordCancelled)}>
                        {tr.share.cancel}
                    </button>
                    {state !== 'recording' ? (
                        <button class="btn btn-main" disabled={!support.ok || saving} aria-busy={saving || undefined} onClick={() => void start()}>
                            {saving && <span class="spinner spinner-sm" aria-hidden="true" />}
                            {saving ? tr.share.saving : tr.share.record}
                        </button>
                    ) : (
                        <button class="btn btn-main" onClick={() => void stop()}>
                            {tr.share.stop(Math.round(seconds))}
                        </button>
                    )}
                </>
            }
        >
            {options.text && <p class="win-lead pre-line">{options.text}</p>}
            {state === 'recording' && (
                <div class="record-level" aria-hidden="true">
                    <div style={{ width: `${width}%` }} />
                </div>
            )}
            {problem && (
                <p class="win-callout m-0" role="alert">
                    {problem}
                </p>
            )}
            <p class="win-note">{'app' in into ? tr.share.micToApp : tr.share.intoFolder(into.folder.split('/').pop() ?? into.folder)}</p>
        </Sheet>
    );
}

function RightsSheet({ pending, close }: { pending: PendingRights; close: (allowed: boolean) => void }) {
    return (
        <Sheet
            title={tr.share.asks(pending.title)}
            about={tr.share.once}
            onClose={() => close(false)}
            footer={
                <>
                    <button class="btn btn-plain" onClick={() => close(false)}>
                        {tr.share.notNow}
                    </button>
                    <button class="btn btn-main" onClick={() => close(true)}>
                        {tr.share.allowAll}
                    </button>
                </>
            }
        >
            <WinSection group>
                {pending.rights.map((right) => (
                    <WinRow key={right.right} title={right.words}>
                        {right.out && <span class="pill pill-yandex">{tr.share.leavesHouseShort}</span>}
                    </WinRow>
                ))}
            </WinSection>
        </Sheet>
    );
}

/** Место окон «Поделиться», записи и «Разрешить всё» — для окошек приложений и экранов пульта: одно на пульт, в корне. */
export function AppShareHost() {
    const [share, setShare] = useState<PendingShare | null>(null);
    const [record, setRecord] = useState<PendingRecord | null>(null);
    const [rights, setRights] = useState<PendingRights | null>(null);
    useEffect(() => {
        showShare = (next) => setShare((current) => (current ? (next?.done({ where: null }), current) : next));
        showRecord = (next) => setRecord((current) => (current ? (next?.done(null, tr.share.recordBusy), current) : next));
        showRights = (next) => setRights((current) => (current ? (next?.done(false), current) : next));
        return () => {
            showShare = null;
            showRecord = null;
            showRights = null;
        };
    }, []);
    return (
        <>
            {rights && (
                <RightsSheet
                    key={`rights-${rights.app}`}
                    pending={rights}
                    close={(allowed) => {
                        rights.done(allowed);
                        setRights(null);
                    }}
                />
            )}
            {share && (
                <ShareSheet
                    key={`share-${share.app ?? share.request.file}`}
                    pending={share}
                    close={(result) => {
                        share.done(result);
                        setShare(null);
                    }}
                />
            )}
            {record && (
                <RecordSheet
                    key={`record-${'app' in record.into ? record.into.app : record.into.folder}`}
                    pending={record}
                    close={(result, error) => {
                        record.done(result, error);
                        setRecord(null);
                    }}
                />
            )}
        </>
    );
}
