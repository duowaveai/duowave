// Что сказать человеку, когда YouTube не дал звука: по ошибке поиска или по выводу yt-dlp. Слова — для колонки,
// без технических подробностей: что случилось и что сделать.

/**
 * Вид беды:
 * - `blocked` — YouTube не открывается из этой сети (в России заблокирован, нет VPN);
 * - `busy` — 429 или «подтвердите, что вы не робот»: с общего адреса провайдера слишком много запросов;
 * - `cipher` — YouTube сменил шифр, yt-dlp ещё не умеет;
 * - `gone` — видео нет или оно закрыто;
 * - `ffmpeg` — на компьютере нет ffmpeg;
 * - `other`.
 */
export function kindOf(error) {
    const message = String(error?.message ?? error ?? '');
    const status = error?.status ?? 0;
    if (error?.kind) return error.kind;
    if (status === 429 || /HTTP Error 429|Too Many Requests|not a bot|confirm you.re not|Sign in to confirm/i.test(message)) {
        return 'busy';
    }
    if (
        error?.network ||
        /timed out|ETIMEDOUT|ECONNRESET|ECONNREFUSED|ENETUNREACH|EHOSTUNREACH|ENOTFOUND|EAI_AGAIN|Connection reset|Network is unreachable|Unable to download API page|Failed to resolve|getaddrinfo|UND_ERR_CONNECT|SSL: |certificate verify failed/i.test(
            message,
        )
    ) {
        return 'blocked';
    }
    // 403 на сами данные: YouTube не пустил к потоку этот адрес — та же беда, что 429.
    if (/HTTP Error 403/i.test(message)) return 'busy';
    if (/Video unavailable|Private video|has been removed|This video is not available|members-only|age-restricted|confirm your age/i.test(message)) {
        return 'gone';
    }
    if (/nsig|n challenge|Signature extraction|Unable to extract|Requested format is not available|JS challenge|player .* not found/i.test(message)) {
        return 'cipher';
    }
    return 'other';
}

const RU = {
    blocked: 'YouTube не открывается из этой сети. Нужен VPN на компьютере — приложение «VPN» или ваш собственный.',
    busy: 'YouTube временно не пускает с вашего адреса: через него идёт много запросов, обычно у провайдера один адрес на многих. Подождите около часа или включите VPN.',
    cipher: 'YouTube поменял защиту, обновляю программу для звука. Попробуйте ещё раз через несколько минут.',
    gone: 'Это видео на YouTube недоступно.',
    ffmpeg: 'Для звука с YouTube на компьютере нужна программа ffmpeg. Поставьте её — например, «brew install ffmpeg» на Mac.',
    other: 'С YouTube не получилось. Попробуйте ещё раз.',
    nothing: 'На YouTube ничего не нашла.',
    ask: 'Что включить с YouTube?',
    playing: (title) => `Включаю «${title}».`,
    added: (title) => `Добавила «${title}» в очередь.`,
};

const EN = {
    blocked: 'YouTube does not open from this network. The computer needs a VPN — the “VPN” app or your own.',
    busy: 'YouTube is turning away your address for now: too many requests come from it, often because the provider shares one address among many. Wait about an hour or turn on a VPN.',
    cipher: 'YouTube changed its protection; updating the sound program. Try again in a few minutes.',
    gone: 'This YouTube video is not available.',
    ffmpeg: 'Sound from YouTube needs ffmpeg on the computer. Install it — for example “brew install ffmpeg” on a Mac.',
    other: 'YouTube did not work out. Try again.',
    nothing: 'Found nothing on YouTube.',
    ask: 'What should I play from YouTube?',
    playing: (title) => `Playing “${title}”.`,
    added: (title) => `Added “${title}” to the queue.`,
};

/** Словарь языка дома: русский по умолчанию, английский для `en…`. */
export function words(locale) {
    return String(locale ?? '').toLowerCase().startsWith('en') ? EN : RU;
}

/** Слова о беде на языке дома. */
export function sayError(error, locale) {
    return words(locale)[kindOf(error)];
}
