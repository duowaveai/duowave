// Шаг «Кто думает» первых шагов (onb/brain.tsx): правда ли «вопросы не уходят из дома», когда отвечает модель на этом
// компьютере. Отдельно от шага, без DOM, — его проверяет scripts/panel-check.ts.

import type { BrainPick, BrainView } from '../../api';

type Seen = Pick<BrainView, 'choice' | 'providers' | 'env'>;

/** Модель, к которой вопрос уйдёт из дома: её сервис и сама модель; `backup` — это «Запасная», а не умная. */
export interface AwayModel {
    title: string;
    model: string;
    backup: boolean;
}

/** Выбранная модель, если вопрос к ней уходит в интернет (облако по ключу, модель из .env в облаке); иначе null. */
function away(view: Seen, pick: BrainPick | null | undefined, backup: boolean): AwayModel | null {
    if (!pick) return null;
    if (pick.provider === 'env') return view.env.cloud ? { title: view.env.title, model: pick.model, backup } : null;
    const row = view.providers.find((provider) => provider.id === pick.provider);
    return row?.cloud ? { title: row.title, model: pick.model, backup } : null;
}

/**
 * Облачная «Запасная» — она ответит, когда модель на компьютере молчит. Её ставит и шлюз сам, как только основная —
 * модель этого компьютера (умолчание 6 блока «Дом», docs/plans/home-and-helpers.md): ответы тогда могут стоить денег, а
 * папки без галочки «Модель может читать имена и файлы» закрыты. Выбор модели на компьютере её не снимает.
 */
export function awayBackup(view: Seen): AwayModel | null {
    return away(view, view.choice.backup, true);
}

/** Модель за выбранной, с которой вопрос уходит из дома: умная в облаке или облачная «Запасная». Нет — null. */
export function awayBehind(view: Seen): AwayModel | null {
    return away(view, view.choice.smart, false) ?? awayBackup(view);
}
