// Один просмотрщик-проигрыватель пульта (docs/plans/feed.md, «Один движок показа»): фото — во весь экран и листать,
// видео — с перемоткой и во весь экран, звук. Его открывают «Папки», окно приложения (`openMedia` в SDK окна),
// карточки — все одним вызовом `openMedia(вещи, { start })`. Один на пульт, в корне (main.tsx), поверх всего, кроме
// вопросов «да/нет». Esc — закрыть, ← → — листать, свайп по фото — тоже листать. Ответ `openMedia` — когда закрыли:
// на чём и на какой секунде видео, чтобы окно приложения продолжило оттуда. `onShare` — кнопка «Поделиться»: пока её
// окно открыто, просмотрщик отходит в сторону (пауза, не виден) и возвращается на ту же вещь.

import { useEffect, useRef, useState } from 'preact/hooks';

import { MediaLink, problemOf } from '../api';
import { tr } from '../i18n';
import { IconChevronLeft, IconChevronRight, IconClose, IconDownload, IconExpand, IconShare } from '../icons';
import { forgetLink, mediaLink, mediaSrc } from './links';
import { MediaStage, fileName } from './stage';
import './media.css';

/** Что показать: файл по адресу шины (ключ берётся при показе, за приложение — если `app`) или готовая ссылка;
 *  `title` — своё название в шапке вместо имени файла. */
export interface MediaItem {
    address?: string;
    link?: MediaLink;
    title?: string;
}

export interface OpenMediaOptions {
    /** Приложение, за которое берутся ключи: его окно попросило. */
    app?: string;
    /** С какой вещи начать. */
    start?: number;
    /** С какой секунды играть первую. */
    at?: number;
    /** «Поделиться» вещью: окно «Поделиться» пульта; ответ — когда его закрыли. Нет — нет и кнопки. */
    onShare?: (link: MediaLink) => Promise<unknown>;
}

/** Чем кончилось: на какой вещи закрыли и на какой секунде было видео или звук. */
export interface MediaClosed {
    index: number;
    at: number | null;
}

interface Opened {
    id: number;
    items: MediaItem[];
    options: OpenMediaOptions;
    done: (closed: MediaClosed) => void;
}

let show: ((next: Opened | null) => void) | null = null;
let current: Opened | null = null;
let opens = 0;

/** Открыть просмотрщик пульта; открытый закрывается. Ответ — когда человек его закрыл. */
export function openMedia(items: MediaItem[], options: OpenMediaOptions = {}): Promise<MediaClosed> {
    if (!items.length) return Promise.reject(new Error(tr.media.nothing));
    if (!show) return Promise.reject(new Error(tr.media.noViewer));
    current?.done({ index: 0, at: null });
    return new Promise((resolve) => {
        opens += 1;
        const opened: Opened = {
            id: opens,
            items,
            options,
            done: (closed) => {
                if (current === opened) {
                    current = null;
                    show?.(null);
                }
                resolve(closed);
            },
        };
        current = opened;
        show?.(opened);
    });
}

/** Место просмотрщика: один на пульт, в корне. */
export function MediaViewerHost() {
    const [opened, setOpened] = useState<Opened | null>(null);
    useEffect(() => {
        show = setOpened;
        return () => {
            show = null;
        };
    }, []);
    return opened ? <Viewer key={opened.id} opened={opened} /> : null;
}

type FullscreenTarget = HTMLDivElement & { webkitRequestFullscreen?: () => void };

function canFullscreen(): boolean {
    const doc = document as Document & { webkitFullscreenEnabled?: boolean };
    return !!(doc.fullscreenEnabled || doc.webkitFullscreenEnabled);
}

function Viewer({ opened }: { opened: Opened }) {
    const { items, options } = opened;
    const [index, setIndex] = useState(() => Math.min(Math.max(options.start ?? 0, 0), items.length - 1));
    // Ссылка вещи или почему её нет; undefined — ещё спрашиваем.
    const [slots, setSlots] = useState<Record<number, MediaLink | string>>({});
    const renewed = useRef(new Set<number>());
    const time = useRef<number | null>(null);
    // С какой секунды играть вещь: первую — откуда попросили, взятую заново — откуда она прервалась.
    const startAt = useRef<Record<number, number | undefined>>({ [index]: options.at });
    const root = useRef<HTMLDivElement>(null);
    const closeButton = useRef<HTMLButtonElement>(null);
    const swipe = useRef<{ x: number; y: number; swiped: boolean } | null>(null);
    // Окно «Поделиться» открыто: просмотрщик не виден и не слышит клавиш — Esc закрывает то окно, не его.
    const [aside, setAside] = useState(false);

    const resolve = (at: number, again = false): void => {
        const item = items[at];
        if (!item) return;
        if (item.link?.url && !again) {
            setSlots((now) => ({ ...now, [at]: item.link! }));
            return;
        }
        if (!item.address) {
            setSlots((now) => ({ ...now, [at]: item.link?.error ?? tr.media.failed }));
            return;
        }
        if (again) forgetLink(item.address, options.app);
        mediaLink(item.address, { app: options.app })
            .then((link) => setSlots((now) => ({ ...now, [at]: link })))
            .catch((e) => setSlots((now) => ({ ...now, [at]: problemOf(e) })));
    };

    // Эта вещь и соседние: листать без ожидания; фото соседей браузер берёт заранее.
    useEffect(() => {
        for (const at of [index, index + 1, index - 1]) {
            if (at >= 0 && at < items.length && slots[at] === undefined) resolve(at);
        }
    }, [index]);

    const close = (): void => {
        if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
        opened.done({ index, at: time.current });
    };
    const go = (step: number): void => {
        const next = index + step;
        if (next < 0 || next >= items.length) return;
        time.current = null;
        setIndex(next);
    };

    useEffect(() => {
        const key = (event: KeyboardEvent): void => {
            if (aside) return;
            // Стрелки на видео и звуке — их перемотка.
            if (event.target instanceof HTMLMediaElement && event.key.startsWith('Arrow')) return;
            if (event.key === 'Escape' && !document.fullscreenElement) {
                event.preventDefault();
                close();
            } else if (event.key === 'ArrowRight') go(1);
            else if (event.key === 'ArrowLeft') go(-1);
        };
        window.addEventListener('keydown', key);
        return () => window.removeEventListener('keydown', key);
    }, [index, aside]);

    // Страница под ним не прокручивается; фокус — на «Закрыть», после — туда, где был.
    useEffect(() => {
        const before = document.activeElement as HTMLElement | null;
        const overflow = document.documentElement.style.overflow;
        document.documentElement.style.overflow = 'hidden';
        closeButton.current?.focus();
        return () => {
            document.documentElement.style.overflow = overflow;
            before?.focus?.();
        };
    }, []);

    const fullscreen = (): void => {
        const element = root.current as FullscreenTarget | null;
        if (!element) return;
        if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
        else if (element.requestFullscreen) void element.requestFullscreen().catch(() => {});
        else element.webkitRequestFullscreen?.();
    };

    const share = (shared: MediaLink): void => {
        if (!options.onShare) return;
        root.current?.querySelectorAll('video, audio').forEach((media) => (media as HTMLMediaElement).pause());
        setAside(true);
        void options
            .onShare(shared)
            .catch(() => {})
            .finally(() => setAside(false));
    };

    const slot = slots[index];
    const link = typeof slot === 'object' ? slot : null;
    const title = items[index]?.title ?? link?.title ?? items[index]?.address?.split('/').pop() ?? '';
    const many = items.length > 1;
    return (
        <div class={`media-viewer${aside ? ' is-aside' : ''}`} ref={root} role="dialog" aria-modal="true" aria-label={tr.media.viewerLabel} aria-hidden={aside}>
            <header class="media-bar">
                <span class="media-title" title={title}>
                    {title}
                </span>
                {many && <span class="media-count">{tr.media.counter(index + 1, items.length)}</span>}
                {link && options.onShare && (
                    <button class="media-button" onClick={() => share(link)} aria-label={tr.media.share} title={tr.media.share}>
                        <IconShare size={18} />
                    </button>
                )}
                {link && (
                    <a class="media-button" href={mediaSrc(link)} download={fileName(link)} aria-label={tr.media.original} title={tr.media.original}>
                        <IconDownload size={18} />
                    </a>
                )}
                {canFullscreen() && (
                    <button class="media-button" onClick={fullscreen} aria-label={tr.media.fullscreen} title={tr.media.fullscreen}>
                        <IconExpand size={18} />
                    </button>
                )}
                <button ref={closeButton} class="media-button" onClick={close} aria-label={tr.media.close} title={tr.media.close}>
                    <IconClose size={18} />
                </button>
            </header>
            <main
                class="media-stage"
                onPointerDown={(event) => {
                    swipe.current = event.target instanceof HTMLMediaElement ? null : { x: event.clientX, y: event.clientY, swiped: false };
                }}
                onPointerUp={(event) => {
                    const from = swipe.current;
                    if (!from || !many) return;
                    const dx = event.clientX - from.x;
                    if (Math.abs(dx) > 60 && Math.abs(dx) > 1.5 * Math.abs(event.clientY - from.y)) {
                        from.swiped = true;
                        go(dx < 0 ? 1 : -1);
                    }
                }}
                onClick={(event) => {
                    // Нажатие мимо фото — закрыть; конец свайпа — не нажатие.
                    if (event.target === event.currentTarget && !swipe.current?.swiped) close();
                }}
            >
                {link ? (
                    <MediaStage
                        key={link.url}
                        link={link}
                        autoPlay
                        at={startAt.current[index]}
                        onTime={(seconds) => {
                            time.current = seconds;
                        }}
                        onStale={() => {
                            if (renewed.current.has(index) || !items[index]?.address) return false;
                            renewed.current.add(index);
                            startAt.current[index] = time.current ?? startAt.current[index];
                            resolve(index, true);
                            return true;
                        }}
                    />
                ) : (
                    <p class="media-note" role="status">
                        {typeof slot === 'string' ? slot : tr.media.opening}
                    </p>
                )}
            </main>
            {many && index > 0 && (
                <button class="media-nav media-prev" onClick={() => go(-1)} aria-label={tr.media.previous} title={tr.media.previous}>
                    <IconChevronLeft size={26} />
                </button>
            )}
            {many && index < items.length - 1 && (
                <button class="media-nav media-next" onClick={() => go(1)} aria-label={tr.media.next} title={tr.media.next}>
                    <IconChevronRight size={26} />
                </button>
            )}
        </div>
    );
}
