// Путь вещи в пульте (docs/plans/voice-creation.md): «сделай…» → агент в фоне → «на примерке» → «Примерка» → «да» →
// на столе. Здесь — общие шаги, которые зовут «+», плитка, «Примерка» и голос пульта: попросить, сказать «готово»,
// поставить. Логика прежняя (мастерская, план установки, одобрение администратором) — пульт собирает её в одно место.

import { DraftFace, DraftView, InstallPlan, MadeDraft, api, problemOf } from '../api';
import { hideStrip, showStrip } from './strip';

/** Слова «сделай…»: просьба как она есть, а «мои дела на сегодня» из поля «+» — как просьба виджета. */
export function asRequest(text: string, face: DraftFace = 'widget'): string {
    const words = text.trim();
    if (/^(сделай|нарисуй|создай|напиши|добавь|давай)\s/i.test(words)) return words;
    const what = face === 'widget' ? 'виджет' : face === 'game' ? 'игру' : face === 'theme' ? 'тему' : 'навык';
    return `Сделай ${what}: ${words}`;
}

/** «виджет» → «Делаю виджет», «игра» → «Делаю игру». */
export function accusative(face: DraftFace): string {
    return face === 'game' ? 'игру' : face === 'theme' ? 'тему' : face === 'skill' ? 'навык' : 'виджет';
}

/** Где агент: облако — янтарём, дома — своё. */
export function agentChip(cloud: boolean | null): { text: string; tone: 'out' | 'own' } | undefined {
    if (cloud === null) return undefined;
    return cloud ? { text: 'агент — облако', tone: 'out' } : { text: 'агент — дома', tone: 'own' };
}

/** «Сделай…» из пульта: та же дорога, что у колонки. Полоса говорит, что агент взял работу; стол свободен. */
export async function askToMake(words: string, cloud: boolean | null, draft?: string): Promise<MadeDraft> {
    const made = await api.makeDraft(words, draft);
    showStrip(
        {
            kind: 'agent',
            head: 'Агент',
            chip: agentChip(cloud),
            text: `${draft ? 'Переделываю' : 'Делаю'} ${accusative(made.kind)}. Скажу, когда будет готово — столом можно пользоваться.`,
        },
        6000,
    );
    return made;
}

/** Что «Готово» говорит людям: строка шлюза, а если её нет — коротко. */
export function readyLine(draft: DraftView): string {
    return draft.readyLine ?? `Готово: «${draft.title ?? draft.id}».`;
}

/** «Готово…» со словами агента — как у колонки: его слова после «Готово: …», до того, что нужно, и вопроса. */
export function readyWithNote(draft: DraftView): string {
    const line = readyLine(draft);
    const note = draft.meta.note?.trim();
    if (!note) return line;
    const at = line.indexOf('». ');
    return at < 0 ? `${line} ${note}` : `${line.slice(0, at + 2)} ${note}${line.slice(at + 2)}`;
}

/** Нужно ли черновику «да» рукой: первая установка или новые права и хосты (решение 5). */
export function needsYes(plan: InstallPlan): boolean {
    return plan.first || plan.added.length > 0 || plan.hostsAdded.length > 0;
}

/** Пустой обязательный ключ: «Вставить ключ» вместо «На стол». */
export function missingKey(draft: DraftView, settings: Record<string, unknown>): boolean {
    return draft.needs.some((need) => need.required && !(typeof settings[need.key] === 'string' && (settings[need.key] as string).trim()));
}

/** Отказ шлюза словами — для полосы и окна. */
export function failed(error: unknown): string {
    return error instanceof Error && !('status' in error) ? error.message : problemOf(error);
}

/** Полоса «Готово…» с подсказками; `fit`, `desk` и прочее — что делают её кнопки. */
export function announceReady(
    draft: DraftView,
    acts: { fit: () => void; desk: () => void; redo: () => void; park: () => void; drop: () => void },
    seconds = 0,
): void {
    const face = draft.meta.kind;
    showStrip({
        kind: 'ready',
        head: 'Агент · готово',
        text: readyWithNote(draft),
        chip: seconds ? { text: `слушаю ещё ${seconds} с`, tone: 'own' } : undefined,
        wait: seconds || undefined,
        chips: [
            { label: 'примерить', act: acts.fit },
            { label: face === 'widget' ? 'на стол' : 'включить', act: acts.desk },
            { label: 'переделай…', act: acts.redo },
            { label: 'в черновики', act: acts.park },
            { label: 'не надо', act: acts.drop },
        ],
        after: `Промолчите — черновик подождёт здесь, на ${face === 'widget' ? 'столе' : 'столе плиткой'}.`,
    });
}

/** Сделано: на столе. Полоса — что колонка теперь знает, и что можно сказать дальше. */
export function announceLanded(title: string, phrases: string[], acts: { redo: () => void; again: () => void; remove: () => void }): void {
    const know = phrases.length > 0 ? ` Колонка теперь тоже знает: «${phrases[0]}».` : '';
    showStrip(
        {
            kind: 'done',
            head: 'Готово',
            text: `«${title}» на столе.${know}`,
            chips: [
                { label: 'убери', act: acts.remove },
                { label: 'переделай…', act: acts.redo },
                { label: 'сделай ещё…', act: acts.again },
            ],
        },
        12_000,
    );
}

export { hideStrip };
