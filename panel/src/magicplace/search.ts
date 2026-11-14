// Поиск MagicPlace (docs/plans/magicplace.md): по загруженному списку, сразу по мере ввода — название и описание на
// всех языках пакета (шлюз присылает их в `texts`), его id, имя источника и слова вида, лица и места на языке пульта.
// Регистр, «ё» и «е», диакритика и знаки не важны; несколько слов — найтись должны все; слово запроса находит слово
// пакета с тем же началом, в том числе в другом падеже: «ленту» — «Лента».

import type { CatalogEntry } from '../api';

/** Текст, как его сравнивает поиск: строчные, «ё» как «е», без диакритики и знаков, одиночные пробелы. */
export function plainText(text: string): string {
    return text
        .toLowerCase()
        .replace(/ё/g, 'е')
        .normalize('NFD')
        .replace(/[̀-ͯ]/g, '')
        .replace(/[^\p{L}\p{N}]+/gu, ' ')
        .trim();
}

/** Слова текста для поиска. */
export function wordsOf(text: string): string[] {
    const plain = plainText(text);
    return plain ? plain.split(' ') : [];
}

/** Подходит ли слово пакета к слову запроса: начинается с него или отличается окончанием (две буквы, от трёх общих). */
export function wordMatches(word: string, asked: string): boolean {
    if (word.startsWith(asked)) return true;
    if (asked.length < 4) return false;
    let common = 0;
    while (common < word.length && common < asked.length && word[common] === asked[common]) common += 1;
    return common >= 3 && common >= asked.length - 2;
}

/** Всё, по чему находится пакет: тексты на всех его языках, id, источник и слова, которые назвал экран (вид, лицо, место). */
export function searchWords(entry: CatalogEntry, extra: string[] = []): string[] {
    const texts = entry.texts?.length ? entry.texts : [{ lang: '', title: entry.title, description: entry.description }];
    const parts = [
        entry.title,
        entry.description ?? '',
        ...texts.flatMap((text) => [text.title, text.description ?? '']),
        entry.id.replace(/-/g, ' '),
        entry.source ?? '',
        ...extra,
    ];
    return [...new Set(parts.flatMap(wordsOf))];
}

/** Подходит ли пакет к запросу: каждое слово запроса находит своё слово пакета. Пустой запрос — подходит всё. */
export function matchesQuery(entry: CatalogEntry, query: string, extra: string[] = []): boolean {
    const asked = wordsOf(query);
    if (asked.length === 0) return true;
    const words = searchWords(entry, extra);
    return asked.every((word) => words.some((own) => wordMatches(own, word)));
}
