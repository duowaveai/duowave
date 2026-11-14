// Путь вещи в пульте (docs/plans/voice-creation.md): «сделай…» → агент в фоне → «на примерке» → «Примерка» → «да» →
// на столе. Здесь — общие шаги, которые зовут «+», плитка, «Примерка» и голос пульта: попросить, сказать «готово»,
// поставить. Логика прежняя (мастерская, план установки, одобрение администратором) — пульт собирает её в одно место.

import { DraftFace, DraftView, InstallPlan, MadeDraft, api, problemOf } from '../api';
import { tr } from '../i18n';
import { hideStrip, showStrip } from './strip';
import { byMode } from '../words';

/** Слова «сделай…»: просьба как она есть, а «мои дела на сегодня» из поля «+» — как просьба виджета. */
export function asRequest(text: string, face: DraftFace = 'widget'): string {
    const words = text.trim();
    // Русские и английские начала просьбы: голос и поле «+» говорят на языке пульта.
    if (/^(сделай|нарисуй|создай|напиши|добавь|давай|make|create|draw|write|add|build|let's)\s/i.test(words)) return words;
    const what = face === 'widget' ? tr.voice.makeWidget : face === 'game' ? tr.voice.makeGame : face === 'theme' ? tr.voice.makeTheme : tr.voice.makeSkill;
    return tr.voice.makeRequest(what, words);
}

/** «виджет» → «Делаю виджет», «игра» → «Делаю игру». */
export function accusative(face: DraftFace): string {
    return face === 'game' ? tr.voice.makeGame : face === 'theme' ? tr.voice.makeTheme : face === 'skill' ? tr.voice.makeSkill : tr.voice.makeWidget;
}

/** Где агент: облако — янтарём, дома — своё. */
export function agentChip(cloud: boolean | null): { text: string; tone: 'out' | 'own' } | undefined {
    if (cloud === null) return undefined;
    return cloud ? { text: tr.voice.agentCloud, tone: 'out' } : { text: tr.voice.agentHome, tone: 'own' };
}

/** «Сделай…» из пульта: та же дорога, что у колонки. Полоса говорит, что агент взял работу; стол свободен. */
export async function askToMake(words: string, cloud: boolean | null, draft?: string): Promise<MadeDraft> {
    const made = await api.makeDraft(words, draft);
    showStrip(
        {
            kind: 'agent',
            head: tr.voice.agent,
            chip: agentChip(cloud),
            text: draft ? tr.voice.makingAgain(accusative(made.kind)) : tr.voice.makingNew(accusative(made.kind)),
        },
        6000,
    );
    return made;
}

/** Что «Готово» говорит людям: строка шлюза, а если её нет — коротко. */
export function readyLine(draft: DraftView): string {
    return draft.readyLine ?? tr.voice.readyLine(draft.title ?? draft.id);
}

/** «Готово…» со словами агента — как у колонки: его слова после «Готово: …», до того, что нужно, и вопроса. */
export function readyWithNote(draft: DraftView): string {
    const line = readyLine(draft);
    const note = draft.meta.note?.trim();
    if (!note) return line;
    // Строка шлюза и запасная кончаются кавычкой с точкой: русской «». » или английской «”. ».
    const at = Math.max(line.indexOf('». '), line.indexOf('”. '));
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
        head: tr.voice.readyHead,
        text: readyWithNote(draft),
        chip: seconds ? { text: tr.voice.listenMore(seconds), tone: 'own' } : undefined,
        wait: seconds || undefined,
        chips: [
            { label: tr.voice.chipFit, act: acts.fit },
            { label: face === 'widget' ? tr.voice.chipDesk : tr.voice.chipEnable, act: acts.desk },
            { label: tr.voice.chipRedo, act: acts.redo },
            { label: tr.voice.chipPark, act: acts.park },
            { label: tr.voice.chipDrop, act: acts.drop },
        ],
        after: face === 'widget' ? tr.voice.afterWidget : tr.voice.afterTile,
    });
}

/** Сделано: на столе. Полоса — что колонка теперь знает, и что можно сказать дальше. */
export function announceLanded(title: string, phrases: string[], acts: { redo: () => void; again: () => void; remove: () => void }): void {
    const know = phrases.length > 0 ? byMode(tr.voice.knowStation(phrases[0]), tr.voice.knowComputer(phrases[0])) : '';
    showStrip(
        {
            kind: 'done',
            head: tr.voice.done,
            text: tr.voice.onDesk(title, know),
            chips: [
                { label: tr.voice.chipRemove, act: acts.remove },
                { label: tr.voice.chipRedo, act: acts.redo },
                { label: tr.voice.chipAgain, act: acts.again },
            ],
        },
        12_000,
    );
}

export { hideStrip };
