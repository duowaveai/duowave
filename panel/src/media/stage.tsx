// Одна вещь движка показа (gateway/media): фото, видео или звук — то, что показывает просмотрщик пульта (viewer.tsx) и
// окно экрана компьютера (screen/page.tsx). Видео и звук — с кнопками браузера: перемотка, громкость, во весь экран.
// Не загрузилось — сначала новая ссылка (шлюз мог перезапуститься и забыть ключ), потом «браузер это не показывает» и
// «Скачать оригинал».

import { useEffect, useRef, useState } from 'preact/hooks';

import { MediaLink } from '../api';
import { tr } from '../i18n';
import { IconDownload, IconMusic } from '../icons';
import { mediaSrc, photoSide } from './links';
import './media.css';

export interface StageProps {
    link: MediaLink;
    /** Видео и звук начинают сами. */
    autoPlay?: boolean;
    /** С какой секунды играть. */
    at?: number;
    /** Пауза снаружи (голосом, агентом): элемент идёт за ней. */
    paused?: boolean;
    onPause?: () => void;
    onPlay?: () => void;
    onEnded?: () => void;
    /** Где сейчас, в секундах. */
    onTime?: (seconds: number) => void;
    /** Файл не загрузился. true — будет новая ссылка, ждать её; false — показать, что не вышло. */
    onStale?: () => boolean;
}

/** Имя файла для «Скачать»: из адреса, с расширением. */
export function fileName(link: MediaLink): string {
    return link.name ?? link.address.split('/').pop() ?? link.title ?? 'file';
}

export function MediaStage(props: StageProps) {
    const { link } = props;
    const [failed, setFailed] = useState(false);
    useEffect(() => setFailed(false), [link.url]);
    const broke = (): void => {
        if (!props.onStale?.()) setFailed(true);
    };
    if (failed) return <Unplayable link={link} />;
    if (link.kind === 'image') return <Photo link={link} onError={broke} />;
    return <Player {...props} onError={broke} />;
}

/** Фото: большой снимок камеры и HEIC — уменьшенным шлюзом; не нарисовалось как есть — ещё раз уменьшенным. */
function Photo({ link, onError }: { link: MediaLink; onError: () => void }) {
    const [side, setSide] = useState(photoSide(link));
    useEffect(() => setSide(photoSide(link)), [link.url]);
    return <img class="media-photo" src={mediaSrc(link, side)} alt={link.title ?? ''} draggable={false} onError={() => (side ? onError() : setSide(2048))} />;
}

function Player({ link, autoPlay, at, paused, onPause, onPlay, onEnded, onTime, onError }: StageProps & { onError: () => void }) {
    const media = useRef<HTMLVideoElement & HTMLAudioElement>(null);
    useEffect(() => {
        const element = media.current;
        if (!element || paused === undefined) return;
        if (paused && !element.paused) element.pause();
        if (!paused && element.paused && !element.ended) void element.play().catch(() => {});
    }, [paused]);
    const own = {
        ref: media,
        src: mediaSrc(link),
        controls: true,
        autoPlay: !!autoPlay && !paused,
        preload: 'metadata' as const,
        onLoadedMetadata: () => {
            if (at && media.current) media.current.currentTime = at;
        },
        onTimeUpdate: () => media.current && onTime?.(media.current.currentTime),
        onPause: () => {
            const element = media.current;
            if (element && !element.ended) onPause?.();
        },
        onPlay: () => onPlay?.(),
        onEnded: () => onEnded?.(),
        onError,
    };
    if (link.kind === 'video') return <video class="media-video" {...own} playsInline />;
    return (
        <div class="media-audio">
            <div class="media-audio-art" aria-hidden="true">
                <IconMusic size={56} />
            </div>
            <p class="media-audio-title">{link.title}</p>
            <audio class="media-audio-player" {...own} />
        </div>
    );
}

function Unplayable({ link }: { link: MediaLink }) {
    return (
        <div class="media-unplayable" role="status">
            <p class="m-0">{link.kind === 'image' ? tr.media.notShown : tr.media.notPlayed}</p>
            <a class="media-button media-button-text" href={mediaSrc(link)} download={fileName(link)}>
                <IconDownload size={18} />
                <span>{tr.media.original}</span>
            </a>
        </div>
    );
}
