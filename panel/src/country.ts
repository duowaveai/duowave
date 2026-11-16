// Страны на экране «Выход в интернет»: флаг из кода и название на языке пульта. Страну дома пульт не спрашивает и не
// угадывает (владелец, 16.11): её спрашивает в своём окне приложение «VPN» и хранит у себя.
// Названия стран — `Intl.DisplayNames` на языке пульта, флаги — эмодзи из кода (Windows покажет буквы — не страшно).

/** Флаг-эмодзи страны: `NL` → 🇳🇱; не код страны — пусто. */
export function flagOf(code: string | null | undefined): string {
    if (!code || !/^[A-Za-z]{2}$/.test(code)) return '';
    const base = 0x1f1e6;
    return String.fromCodePoint(...code.toUpperCase().split('').map((letter) => base + letter.charCodeAt(0) - 65));
}

const names = new Map<string, Intl.DisplayNames | null>();

/** Название страны на языке пульта; браузер не знает — сам код. */
export function countryName(code: string, locale: string): string {
    let display = names.get(locale);
    if (display === undefined) {
        try {
            display = new Intl.DisplayNames([locale], { type: 'region' });
        } catch {
            display = null;
        }
        names.set(locale, display);
    }
    try {
        return display?.of(code.toUpperCase()) ?? code;
    } catch {
        return code;
    }
}
