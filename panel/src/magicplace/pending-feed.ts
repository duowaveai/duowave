// Лента выбора, закрытая колонке (22.11): у тестера Станция Мини 2 не открывала ленты за Cloudflare, а пульт без конца
// писал «готовлю у колонки». Каталог колонки говорит о каждой ленте, дозвалась ли она (`feeds`), — по нему пульт и
// понимает, что ждать нечего, пока лента не откроется. Без интерфейса: проверяется в panel-check.

import type { CatalogSource } from '../api';

/** Один и тот же адрес ленты: с `catalog.json` на конце или без, со слешем или без. */
export function sameFeed(a: string, b: string): boolean {
    const bare = (url: string) => url.trim().replace(/\/catalog\.json$/i, '').replace(/\/+$/, '').toLowerCase();
    return bare(a) === bare(b);
}

/**
 * Почему колонка не может скачать ленту `url`, её словами (`any` — когда она не сказала); undefined — может, или о
 * ленте ещё ничего не известно (колонка постарше или ещё не читала её).
 */
export function blockedOf(feeds: CatalogSource[] | undefined, url: string, any: string): string | undefined {
    const feed = feeds?.find((item) => sameFeed(item.url, url));
    return feed && (feed.offline || feed.closed) ? (feed.problem ?? any) : undefined;
}
