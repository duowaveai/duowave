// Шина «Связи» словами для людей (docs/plans/bus.md): ступени, режимы, кто поставил связь, чем кончилось письмо.
// Те же слова, что у шлюза (gateway/src/bus/level.rs): у файлов «зовёт» — это «читает».

import type { BusLevel, BusLinkSource, BusMode } from '../api';

/** Ступени по порядку: каждая держит те, что ниже. */
export const LEVELS: BusLevel[] = ['none', 'see', 'listen', 'call', 'edit'];

/** Ступень, как её читают о связи: «Покупки — зовёт — Заметки». */
export function levelWord(level: BusLevel, address = ''): string {
    const files = address.startsWith('files/');
    switch (level) {
        case 'none':
            return 'не знает';
        case 'see':
            return 'видит';
        case 'listen':
            return 'слышит';
        case 'call':
            return files ? 'читает' : 'зовёт';
        case 'edit':
            return files ? 'меняет' : 'правит';
    }
}

/** Что даёт ступень — подсказка к выбору. */
export function levelHint(level: BusLevel, address = ''): string {
    const files = address.startsWith('files/');
    switch (level) {
        case 'none':
            return files ? 'папки для него нет' : 'для него его нет';
        case 'see':
            return files ? 'видит, какие там файлы' : 'знает, что оно есть и что умеет';
        case 'listen':
            return 'слышит его новости';
        case 'call':
            return files ? 'читает файлы' : 'зовёт его умения';
        case 'edit':
            return files ? 'кладёт, меняет и удаляет файлы' : 'зовёт всё, и правку тоже';
    }
}

export const MODE_TITLES: Record<BusMode, string> = {
    isolated: 'Отдельно',
    rules: 'По правилам',
    free: 'Свободно',
};

export const MODE_HINTS: Record<BusMode, string> = {
    isolated: 'Ни с кем не общается, и к нему никто — только вы в пульте.',
    rules: 'Только то, что приложение попросило и вы разрешили. Впервые тянется — спросит «да», как в телефоне.',
    free: 'Видит и зовёт всё открытое. Личное и опасное — всё равно через вас.',
};

export const SOURCE_TITLES: Record<BusLinkSource, string> = {
    owner: 'вы поставили',
    answer: 'ваше «да»',
    refused: 'ваше «нет»',
    install: 'при установке',
};

/** Чем кончилось письмо в журнале. */
export const OUTCOME_TITLES: Record<string, string> = {
    ok: 'прошло',
    failed: 'не вышло',
    asked: 'ждёт «да»',
    denied: 'нет связи',
    notFound: 'не нашёл',
    fuse: 'остановлено',
    yes: '«да»',
    no: '«нет»',
};

/** Что делали: позвал, рассказал новость, спросил словами, посмотрел, ответили, доставили. */
export const KIND_TITLES: Record<string, string> = {
    call: 'зовёт',
    tell: 'новость',
    ask: 'спрашивает',
    list: 'смотрит',
    answer: 'ответ',
    deliver: 'доставка',
};

/** Опасности словами (gateway/src/bus/danger.rs). */
export const DANGER_TITLES: Record<string, string> = {
    money: 'деньги',
    delete: 'удаление',
    outside: 'наружу',
    locks: 'замки',
    camera: 'камера',
    access: 'доступ',
};

/** Кто начал цепочку, по метке журнала (gateway/src/bus/trail.rs): `user:…`, `household`, `apiKey:…`, `app:radio`,
 *  `system`. */
export function rootWord(root: string): string {
    if (root.startsWith('app:')) return `приложение ${root.slice(4)}`;
    if (root === 'system') return 'шлюз';
    if (root === 'household') return 'голос дома';
    if (root.startsWith('user:')) return 'человек';
    if (root.startsWith('apiKey:')) return 'ключ';
    return root;
}
