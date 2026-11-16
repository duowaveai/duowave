// Выбор «Запасной» в «Мозге» без DOM (panel/scripts/panel-check.ts проверяет его как есть). Значение списка — строка:
// модель — «провайдер\0модель»; пустое — «пусть выберет шлюз» у шлюза с блоком «Дом» и «не нужна» у шлюза без него;
// NO_BACKUP — «не нужна» рядом с «пусть выберет шлюз» (решение 18 плана docs/plans/home-and-helpers.md).

import type { BrainPick, BrainView } from '../api';

/** «Не нужна» рядом с «пусть выберет шлюз»: ни с одной моделью не совпадёт — у провайдера всегда есть имя. */
export const NO_BACKUP = '\u0000none';

export const pickValue = (pick: BrainPick | null | undefined): string => (pick ? `${pick.provider}\u0000${pick.model}` : '');

export const parsePick = (value: string): BrainPick | null => {
    if (!value) return null;
    const [provider, model] = value.split('\u0000');
    return { provider, model };
};

/**
 * Что стоит в списке «Запасная». У шлюза с блоком: выбор самого шлюза — на «пусть выберет шлюз» (подпись называет
 * модель), модель человека — на ней, отказ — на «не нужна», ничего — на «пусть выберет шлюз». У шлюза без блока
 * (0.13.0, 0.14.0 — окно колонки) — модель или «не нужна».
 */
export const backupShown = (choice: BrainView['choice']): string => {
    if (!choice.autoBackup) return pickValue(choice.backup);
    if (choice.backupBy === 'gateway') return '';
    if (choice.backup) return pickValue(choice.backup);
    return choice.backupBy === 'declined' ? NO_BACKUP : '';
};

/** Пункт списка без модели: его значение, слово (ключ `tr.brain`) и модель, которую шлюз уже выбрал сам. */
export interface BlankBackup {
    value: string;
    word: 'optGatewayChooses' | 'optNotNeeded';
    model?: string;
}

/** Пункты без модели по порядку: у шлюза с блоком — «пусть выберет шлюз» и «не нужна», у шлюза без него — «не нужна». */
export const blankBackups = (choice: BrainView['choice']): BlankBackup[] =>
    choice.autoBackup
        ? [
              { value: '', word: 'optGatewayChooses', model: choice.backupBy === 'gateway' ? choice.backup?.model : undefined },
              { value: NO_BACKUP, word: 'optNotNeeded' },
          ]
        : [{ value: '', word: 'optNotNeeded' }];

/**
 * Что уходит шлюзу: модель; `'auto'` — «пусть выберет шлюз» (отказ, модель человека и прежний выбор шлюза уходят,
 * шлюз выбирает заново сам); `null` — «не нужна» (шлюз больше не ставит её сам). Шлюзу без блока (`autoBackup` нет)
 * пустое значение — `null`, как в нём самом.
 */
export const backupOf = (value: string, autoBackup: boolean): BrainPick | null | 'auto' =>
    value === '' ? (autoBackup ? 'auto' : null) : value === NO_BACKUP ? null : parsePick(value);

/**
 * «Пусть выберет шлюз», а запасной нет и шлюз её сейчас не поставит: основная модель — не у помощника (облако, своя
 * подписка), а шлюз ставит запасную сам только за моделью помощника (`mainOnHelper` — быстрая или умная у помощника,
 * в том числе у сервера моделей этого компьютера). Тогда «Мозг» говорит это строкой: молча — значит «запасная будет».
 */
export const backupIdle = (choice: BrainView['choice'], mainOnHelper: boolean): boolean =>
    !!choice.autoBackup && !choice.backup && choice.backupBy !== 'declined' && !mainOnHelper;

/**
 * Куда уйдёт вопрос, если ответит запасная: её название, дома она или в интернете, и Турбо, на чьей модели она (тогда
 * она уснёт вместе с ним).
 */
export function backupPlace(view: Pick<BrainView, 'choice' | 'env' | 'providers'>): { title: string; cloud: boolean; helper?: string } | null {
    const backup = view.choice.backup;
    if (!backup) return null;
    if (backup.provider === 'env') return { title: view.env.title, cloud: view.env.cloud };
    const provider = view.providers.find((entry) => entry.id === backup.provider);
    return provider ? { title: provider.title, cloud: !!provider.cloud, helper: provider.helper?.id } : null;
}
