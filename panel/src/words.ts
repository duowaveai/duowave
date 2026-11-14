// Слова режима (docs/plans/mic-mode.md, «Без Алисы и колонки»): общие экраны пульта говорят «колонка» и «Алиса», пока
// с DuoWave говорят через колонку, и «DuoWave» — когда микрофоном этого компьютера: там колонки, Алисы и Яндекса нет.
// Надпись, которая зависит от режима, берётся здесь — `byMode(с колонкой, без неё)` — и читается при каждой
// отрисовке: режим меняется без перезагрузки пульта.

import { tr } from './i18n';
import { onComputer } from './voice-input';

/** Надпись по режиму: первая — когда говорят через колонку, вторая — микрофоном компьютера. */
export function byMode<T>(station: T, computer: T): T {
    return onComputer() ? computer : station;
}

type Case = 'nom' | 'gen' | 'dat' | 'acc' | 'ins' | 'pre';

/**
 * С кем говорят: «колонка» в нужном падеже или «DuoWave» — оно не склоняется. `capital` — с большой буквы в начале
 * предложения.
 */
export function device(form: Case = 'nom', capital = false): string {
    if (onComputer()) return 'DuoWave';
    const word = tr.core.device(form);
    return capital ? word[0].toUpperCase() + word.slice(1) : word;
}

/** Встроенный помощник: «Алиса» у колонки, «ДиВи» — когда говорят микрофоном компьютера. */
export function assistant(): string {
    return byMode(tr.core.assistantStation, tr.core.assistantComputer);
}

/** Пример фразы: колонке — с её словом и её умением, компьютеру — то, что он умеет без колонки. */
export function examplePhrase(): string {
    return byMode(tr.core.examplePhraseStation, tr.core.examplePhraseComputer);
}

/** Чьи часы: колонки или этого компьютера. */
export function clockOwner(): string {
    return byMode(tr.core.clockOwnerStation, tr.core.clockOwnerComputer);
}
