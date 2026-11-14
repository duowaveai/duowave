// Видео, звук и картинки своей папки — окошку приложения, тем же движком показа, что у всего пульта (gateway/media,
// «Один движок показа» в docs/plans/feed.md). `<video src>` ключ входа не несёт, а у окошка нет своего происхождения,
// поэтому пульт берёт у шлюза ключ к файлу за это приложение — `files/app/<id>/<путь>`, его право проверяет шина — и
// отдаёт окошку адрес `/media/v/<ключ>`: шлюз отдаёт файл с диска с перемоткой (Range), целиком в память не грузит,
// ничего, что браузер запустит. Ключ живёт шесть часов; новый пульт берёт заранее (media/links.ts).

import { tr } from '../i18n';
import { mediaLink, mediaSrc } from '../media/links';

/** Путь внутри своей папки: части без точки впереди, без «..». */
export function mediaPath(name: string): string {
    const parts = String(name ?? '')
        .split('/')
        .filter((part) => part.length > 0);
    if (!parts.length || parts.some((part) => part.startsWith('.') || part.includes('\\'))) {
        throw new Error(tr.media.ownPath);
    }
    return parts.join('/');
}

/** Адрес шины файла своей папки приложения; адрес другой папки (`files/…`) — как есть, его проверит шина. */
export function appAddress(app: string, name: string): string {
    const given = String(name ?? '');
    return given.startsWith('files/') ? given : `files/app/${app}/${mediaPath(given)}`;
}

/** Адрес файла папки приложения `app` для его окошка (и для «Поделиться»). */
export async function mediaUrl(app: string, name: string): Promise<string> {
    return mediaSrc(await mediaLink(`files/app/${app}/${mediaPath(name)}`, { app }));
}
