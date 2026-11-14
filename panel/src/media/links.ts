// Ключи движка показа (gateway/media, «Один движок показа» в docs/plans/feed.md): адрес файла для `<img>`, `<video>`,
// `<audio>` — такие теги не несут ключ входа. Пульт просит ключ у шлюза для себя или за приложение (его окно, его
// карточка): права проверяет шина. Ключ помнится, пока жив, — тот же файл, тот же адрес, и кэш браузера держит; за
// минуту до конца берётся новый. Шлюз перезапустился и ключи забыл — тег не загрузится: `forgetLink` и спросить снова.

import { useEffect, useState } from 'preact/hooks';

import { MediaLink, api } from '../api';
import { tr } from '../i18n';
import { gw } from '../via';

interface Kept {
    link: MediaLink;
    until: number;
    probed: boolean;
}

const kept = new Map<string, Kept>();
/** Адресов в одном вопросе шлюзу, самое большее (gateway/media/routes.rs). */
const ADDRESSES_MOST = 64;
/** Имён папки в одном вопросе (страница папки — до 500). */
const NAMES_MOST = 500;

const keyOf = (app: string | undefined, address: string): string => `${app ?? ''}|${address}`;

function fresh(entry: Kept | undefined, probe: boolean): entry is Kept {
    return !!entry && entry.until > Date.now() + 60_000 && (!probe || entry.probed);
}

function keep(app: string | undefined, link: MediaLink, probe: boolean): void {
    if (!link.url || link.error) return;
    kept.set(keyOf(app, link.address), { link, until: Date.now() + (link.expiresInS ?? 0) * 1000, probed: probe });
}

export interface LinkOptions {
    /** Приложение, за которое спрашивают: его окно или карточка. */
    app?: string;
    /** И сколько длится (несколько КБ каждого файла читаются). */
    probe?: boolean;
}

/** Ссылки на файлы по адресам шины (`files/…`): живые — из памяти, остальные — у шлюза. Нет ссылки — `error`. */
export async function mediaLinks(addresses: string[], options: LinkOptions = {}): Promise<MediaLink[]> {
    const probe = !!options.probe;
    const missing = [...new Set(addresses.filter((address) => !fresh(kept.get(keyOf(options.app, address)), probe)))];
    const failed = new Map<string, MediaLink>();
    for (let at = 0; at < missing.length; at += ADDRESSES_MOST) {
        const { links } = await api.mediaLinks({ app: options.app, addresses: missing.slice(at, at + ADDRESSES_MOST), probe });
        for (const link of links) {
            if (link.url && !link.error) keep(options.app, link, probe);
            else failed.set(link.address, link);
        }
    }
    return addresses.map((address) => kept.get(keyOf(options.app, address))?.link ?? failed.get(address) ?? { address, error: tr.media.failed });
}

/** Ссылки на файлы одной папки по именам — одним письмом шины на страницу. */
export async function folderLinks(folder: string, names: string[], options: LinkOptions = {}): Promise<MediaLink[]> {
    const probe = !!options.probe;
    const address = (name: string): string => `${folder}/${name}`;
    const missing = names.filter((name) => !fresh(kept.get(keyOf(options.app, address(name))), probe));
    const failed = new Map<string, MediaLink>();
    for (let at = 0; at < missing.length; at += NAMES_MOST) {
        const { links } = await api.mediaLinks({ app: options.app, folder, names: missing.slice(at, at + NAMES_MOST), probe });
        for (const link of links) {
            if (link.url && !link.error) keep(options.app, link, probe);
            else failed.set(link.address, link);
        }
    }
    return names.map((name) => kept.get(keyOf(options.app, address(name)))?.link ?? failed.get(address(name)) ?? { address: address(name), error: tr.media.failed });
}

/** Одна ссылка; нет её — ошибка словами. */
export async function mediaLink(address: string, options: LinkOptions = {}): Promise<MediaLink> {
    const [link] = await mediaLinks([address], options);
    if (!link?.url) throw new Error(link?.error ?? tr.media.failed);
    return link;
}

/** Ссылка, которую шлюз не узнал (перезапустился): следующий вопрос возьмёт новую. */
export function forgetLink(address: string, app?: string): void {
    kept.delete(keyOf(app, address));
}

/** Адрес для тега: через проход помощника, когда пульт смотрит на колонку; `size` — картинка файла (фото меньше, кадр
 *  видео). */
export function mediaSrc(link: MediaLink, size?: number): string {
    const url = link.url ?? '';
    return gw(size ? `${url}${url.includes('?') ? '&' : '?'}size=${size}` : url);
}

/** Фото, которое лучше показать уменьшенным: браузер его не рисует (HEIC, TIFF) или это большой снимок камеры. Гифка,
 *  PNG и WebP идут как есть — у них бывают движение и прозрачность. */
export function photoSide(link: MediaLink, side = 2048): number | undefined {
    if (link.kind !== 'image') return undefined;
    if (link.shown === false) return side;
    const camera = link.mime === 'image/jpeg' || link.mime === 'image/avif' || link.mime === 'image/bmp';
    return camera && (link.size ?? 0) > 1_500_000 ? side : undefined;
}

/** Длительность словами часов: «3:07», «1:02:45». */
export function clock(seconds: number | undefined): string {
    if (seconds == null || !Number.isFinite(seconds)) return '';
    const whole = Math.max(0, Math.round(seconds));
    const hours = Math.floor(whole / 3600);
    const minutes = Math.floor((whole % 3600) / 60);
    const rest = String(whole % 60).padStart(2, '0');
    return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${rest}` : `${minutes}:${rest}`;
}

/**
 * Адрес для `<img>` по адресу шины — для себя или за приложение (`app`: его карточка): `size` — уменьшенная. `retry` —
 * тег не загрузился: один раз взять новый ключ (шлюз мог перезапуститься), потом `failed`.
 */
export function useMediaSrc(address: string | null | undefined, options: { app?: string; size?: number } = {}): { src: string | null; failed: boolean; retry: () => void } {
    const { app, size } = options;
    const [shown, setShown] = useState<{ src: string | null; failed: boolean }>({ src: null, failed: false });
    const [round, setRound] = useState(0);
    useEffect(() => setRound(0), [address, app]);
    useEffect(() => {
        if (!address) {
            setShown({ src: null, failed: false });
            return;
        }
        let alive = true;
        mediaLink(address, { app })
            .then((link) => alive && setShown({ src: mediaSrc(link, size), failed: false }))
            .catch(() => alive && setShown({ src: null, failed: true }));
        return () => {
            alive = false;
        };
    }, [address, app, size, round]);
    const retry = (): void => {
        if (!address || round > 0) {
            setShown({ src: null, failed: true });
            return;
        }
        forgetLink(address, app);
        setRound(1);
    };
    return { ...shown, retry };
}
