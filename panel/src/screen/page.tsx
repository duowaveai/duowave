// Окно на экране компьютера (gateway/src/screen, desktop/src-tauri/src/screen.rs): приложение DuoWave открывает его своим
// окном поверх всех по просьбе голоса, агента или приложения — фото, видео, звук. Временное закрывается, когда
// следующая просьба открыла свои окна или видео доиграло; «Оставить» держит его, пока не закроют. Нажатия идут в шлюз,
// он же говорит окну, что его поставили на паузу или закрыли голосом.

import { useEffect, useRef, useState } from 'preact/hooks';

import { ScreenAction, ScreenWindow, api, problemOf } from '../api';
import { tr } from '../i18n';
import { IconClose, IconPause, IconPin, IconPlay } from '../icons';
import { useLive } from '../live';
import './screen.css';

/** Сторона фото, какую просить у шлюза: окно бывает и во весь экран; больше шлюз не делает (preview.rs, SIDE_MOST). */
const PHOTO_SIDE = 2048;

export function ScreenPage({ id }: { id: string }) {
    // undefined — ещё не прочитано, null — окна уже нет.
    const [shown, setShown] = useState<ScreenWindow | null | undefined>(undefined);
    const [error, setError] = useState<string | null>(null);
    const [version, setVersion] = useState(0);
    useLive(true, (name) => {
        if (name === 'screen.changed' || name === 'hello') setVersion((value) => value + 1);
    });
    useEffect(() => {
        let alive = true;
        api.screen()
            .then((view) => {
                if (!alive) return;
                setShown(view.windows.find((window) => window.id === id) ?? null);
                setError(null);
            })
            .catch((e) => alive && setError(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [id, version]);
    useEffect(() => {
        if (shown) document.title = shown.title;
    }, [shown?.title]);
    const act = (action: ScreenAction): void => {
        void api
            .screenAct(id, action)
            .then((answer) => setShown(answer.windows.find((window) => window.id === id) ?? null))
            .catch((e) => setError(problemOf(e)));
    };
    // Esc — закрыть, пробел — пауза и дальше.
    useEffect(() => {
        const key = (event: KeyboardEvent): void => {
            if (!shown) return;
            if (event.key === 'Escape') act('close');
            else if (event.key === ' ' && shown.kind !== 'image' && !(event.target instanceof HTMLMediaElement)) {
                event.preventDefault();
                act(shown.paused ? 'resume' : 'pause');
            }
        };
        window.addEventListener('keydown', key);
        return () => window.removeEventListener('keydown', key);
    }, [shown?.paused, shown?.kind, id]);

    if (shown === undefined)
        return <div class="screen-page">{error && <p class="screen-note">{error}</p>}</div>;
    if (shown === null)
        return (
            <div class="screen-page">
                <p class="screen-note">{tr.core.screenClosed}</p>
            </div>
        );
    return (
        <div class={`screen-page screen-${shown.kind}`}>
            <header class="screen-bar">
                <span class="screen-title" title={shown.from ?? shown.title}>
                    {shown.title}
                </span>
                {shown.kind !== 'image' && (
                    <button class="screen-button" aria-label={shown.paused ? tr.core.screenResume : tr.core.screenPause} onClick={() => act(shown.paused ? 'resume' : 'pause')}>
                        {shown.paused ? <IconPlay size={16} /> : <IconPause size={16} />}
                    </button>
                )}
                <button
                    class={`screen-button${shown.kept ? ' screen-kept' : ''}`}
                    aria-pressed={shown.kept}
                    title={shown.kept ? tr.core.screenKeptTitle : tr.core.screenKeepTitle}
                    onClick={() => act(shown.kept ? 'temporary' : 'keep')}
                >
                    <IconPin size={16} />
                    <span>{shown.kept ? tr.core.screenKept : tr.core.screenKeep}</span>
                </button>
                <button class="screen-button" aria-label={tr.core.screenClose} onClick={() => act('close')}>
                    <IconClose size={16} />
                </button>
            </header>
            <main class="screen-content">
                {shown.kind === 'image' ? <img src={`${shown.url}?size=${PHOTO_SIDE}`} alt={shown.title} /> : <Player shown={shown} onAct={act} />}
            </main>
            {error && <p class="screen-note screen-error">{error}</p>}
        </div>
    );
}

/** Видео или звук: пауза голосом и агентом приходит списком шлюза — элемент идёт за ним; свои нажатия — в шлюз. */
function Player({ shown, onAct }: { shown: ScreenWindow; onAct: (action: ScreenAction) => void }) {
    const media = useRef<HTMLVideoElement & HTMLAudioElement>(null);
    useEffect(() => {
        const element = media.current;
        if (!element) return;
        if (shown.paused && !element.paused) element.pause();
        if (!shown.paused && element.paused && !element.ended) void element.play().catch(() => {});
    }, [shown.paused]);
    const own = {
        ref: media,
        src: shown.url,
        autoPlay: !shown.paused,
        controls: true,
        onPause: () => {
            const element = media.current;
            if (element && !element.ended && !shown.paused) onAct('pause');
        },
        onPlay: () => {
            if (shown.paused) onAct('resume');
        },
        onEnded: () => onAct('ended'),
    };
    if (shown.kind === 'video') return <video {...own} playsInline />;
    return (
        <div class="screen-audio">
            <audio {...own} />
        </div>
    );
}
