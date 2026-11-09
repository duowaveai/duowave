// Картинки и звук пакетов: аватар персонажа, превью каталога, фразы-пробники. Шлюз отдаёт их по ключу входа, а
// `<img src>` и `<audio src>` ключ не передают — поэтому fetch в blob (api.media), один раз на адрес за жизнь страницы.

import { useEffect, useId, useState } from 'preact/hooks';

import { api } from '../api';
import { useAudio } from '../audio';

const cache = new Map<string, Promise<string | null>>();

function load(url: string): Promise<string | null> {
    let entry = cache.get(url);
    if (!entry) {
        entry = api
            .media(url)
            .then((blob) => URL.createObjectURL(blob))
            .catch(() => {
                cache.delete(url); // не вышло — попробуем при следующем показе
                return null;
            });
        cache.set(url, entry);
    }
    return entry;
}

/** Адрес картинки шлюза, годный для `<img src>`; null — пока грузится, не загрузилась или адреса нет. */
export function useMediaUrl(url?: string | null): string | null {
    const [src, setSrc] = useState<string | null>(null);
    useEffect(() => {
        setSrc(null);
        if (!url) return;
        let alive = true;
        void load(url).then((value) => alive && setSrc(value));
        return () => {
            alive = false;
        };
    }, [url]);
    return src;
}

/** Цвет из пакета — только #RGB…#RRGGBBAA: значение идёт в стиль. */
export const safeColor = (color?: string | null): string | undefined => (color && /^#[0-9a-f]{3,8}$/i.test(color) ? color : undefined);

/**
 * Знак встроенного голосового помощника колонки (персонаж «Алиса» шлюза): звуковая волна — пять скруглённых столбиков
 * на градиенте от пурпурного к фиолетовому. Свой рисунок, не логотип Яндекса: их знаки пульт не рисует (THIRD_PARTY.md).
 */
export function VoiceMark() {
    const id = useId();
    return (
        <svg class="char-mark" viewBox="0 0 96 96" aria-hidden="true">
            <defs>
                <linearGradient id={id} x1="0.15" y1="0.95" x2="0.85" y2="0.1">
                    <stop offset="0" stop-color="#D02CF2" />
                    <stop offset="0.55" stop-color="#7A2CFF" />
                    <stop offset="1" stop-color="#4A2BFF" />
                </linearGradient>
            </defs>
            <circle cx="48" cy="48" r="48" fill={`url(#${id})`} />
            <g fill="#FFFFFF">
                <rect x="23" y="40" width="8" height="16" rx="4" />
                <rect x="35" y="31" width="8" height="34" rx="4" />
                <rect x="47" y="24" width="8" height="48" rx="4" />
                <rect x="59" y="33" width="8" height="30" rx="4" />
                <rect x="71" y="41" width="8" height="14" rx="4" />
            </g>
        </svg>
    );
}

/** Персонаж, у которого вместо буквы свой знак: встроенная «Алиса» — волна. */
export const markOf = (character?: { id: string; builtin: boolean } | null): 'voice' | undefined =>
    character?.builtin && character.id === 'alice' ? 'voice' : undefined;

/** Аватар: картинка пакета, а без неё — свой знак (mark) или первая буква имени на цвете персонажа. */
export function Avatar({
    src,
    title,
    color,
    size = 'md',
    mark,
}: {
    src?: string | null;
    title: string;
    color?: string | null;
    size?: 'sm' | 'md' | 'lg';
    mark?: 'voice';
}) {
    const url = useMediaUrl(src);
    const tint = safeColor(color);
    const cls = `${size === 'sm' ? 'char-avatar char-avatar-sm' : size === 'lg' ? 'char-avatar char-avatar-lg' : 'char-avatar'}${!url && mark ? ' char-avatar-mark' : ''}`;
    return (
        <span class={cls} style={tint ? { '--char-color': tint } : undefined} aria-hidden="true">
            {url ? <img src={url} alt="" /> : mark === 'voice' ? <VoiceMark /> : (title.trim()[0] ?? '?').toUpperCase()}
        </span>
    );
}

/** ▶ послушать фразу-пробник: слышно до установки и без модели. Звучит одно за раз. */
export function PlayButton({ id, url, label, className = 'char-play' }: { id: string; url?: string | null; label: string; className?: string }) {
    const audio = useAudio();
    if (!url) return null;
    const key = `media:${id}:${url}`;
    const playing = audio.playing === key;
    return (
        <button
            type="button"
            class={className}
            aria-pressed={playing}
            aria-label={playing ? 'Остановить' : `Послушать: ${label}`}
            onClick={(event) => {
                event.stopPropagation();
                void audio.toggle(key, () => api.media(url));
            }}
        >
            {playing ? '■' : '▶'}
        </button>
    );
}
