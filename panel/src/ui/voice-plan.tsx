// Модель голоса на компьютере-Турбо (проверка начисто 18.11, п. 40): строка выбора модели, полоса хода и выбор целиком —
// с местом и ОЗУ до кнопки. Один кусок на всех: мастер «Свой голос» (screens/voice-lab.tsx), мастер установки персонажа
// (magicplace/wizard.tsx) и окно «нужна модель» (ui/needs.tsx), когда персонажа выбирают позже в «Персонаже».

import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import type { HomeVoicePlan, VoicePlanEngine } from '../api';
import { formatNumber, tr } from '../i18n';
import { WinRow, WinSection } from './sheet';

const fixed1 = (value: number): string => formatNumber(value, { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/** ГБ из МБ плана Турбо — одна цифра после запятой, по 1024, как размеры везде в пульте (ui/needs.tsx sizeText). */
export const planGb = (mb: number | null | undefined): string => fixed1((mb ?? 0) / 1024);

/** Модели, на которых голос персонажа может пойти на этом Турбо. */
export const usableEngines = (plan: HomeVoicePlan | null | undefined): VoicePlanEngine[] =>
    (plan?.plan?.engines ?? []).filter((engine) => engine.supported && engine.way !== 'none');

/** Помещается на диск компьютера: стоит уже или Турбо не сказал «не влезет». ОЗУ впритык — не запрет, только предупреждение. */
export const fitsDisk = (engine: VoicePlanEngine): boolean => engine.state === 'ready' || engine.diskFits !== false;

/** Ни одна подходящая модель не помещается: голос не ставить, говорит запасной. */
export const noRoom = (plan: HomeVoicePlan | null | undefined): boolean => {
    const list = usableEngines(plan);
    return list.length > 0 && !list.some(fitsDisk);
};

/** Сколько места займёт модель. */
const needMb = (engine: VoicePlanEngine): number => engine.totalMb ?? engine.diskMb;

/**
 * Модель по умолчанию: та, что ставится сейчас, выбор владельца у центра, та, что уже стоит, выбор Турбо, советуемая,
 * первая — из тех, что помещаются на диск. Не помещается ни одна — null: выбрать нечего, кнопка установки модели не
 * запускает.
 */
export function defaultEngine(plan: HomeVoicePlan | null | undefined): string | null {
    const list = usableEngines(plan).filter(fitsDisk);
    const installing = list.filter((engine) => engine.state === 'installing');
    // Что уже ставится — первым (21.11: «Сейчас» говорил желание, а качалась другая): для голоса этого персонажа Турбо
    // называет её в `plan.chosen`. Дальше выбор владельца (центр помнит), уже стоящая (владелец 18.11: «поставить на
    // готовый»), и только потом то, что выбрал бы сам Турбо, и советуемая.
    return (
        installing.find((engine) => engine.id === plan?.plan?.chosen)?.id ??
        installing[0]?.id ??
        list.find((engine) => engine.id === plan?.chosen)?.id ??
        list.find((engine) => engine.state === 'ready')?.id ??
        list.find((engine) => engine.id === plan?.plan?.chosen)?.id ??
        list.find((engine) => engine.recommended)?.id ??
        list[0]?.id ??
        null
    );
}

/** Строка модели в выборе: стоит / качается / сколько скачается и ОЗУ, и как под неё встанет голос. */
export function engineStatus(engine: VoicePlanEngine): string {
    const t = tr.wizard;
    const state =
        engine.state === 'ready'
            ? t.engineReady
            : engine.state === 'installing'
              ? t.engineLoading(engine.percent ?? 0)
              : !fitsDisk(engine)
                ? t.engineNoRoom(planGb(needMb(engine)), planGb(engine.freeDiskMb))
                : t.engineMissing(planGb(needMb(engine)), planGb(engine.ramMb));
    const way = t.engineWay[engine.way];
    return way ? `${state} · ${way}` : state;
}

/** Полоса прогресса задачи: этап, процент, память. */
export function JobBar({ label, percent, memoryMb }: { label: string; percent: number; memoryMb?: number }) {
    return (
        <div class="flex flex-col gap-6">
            <div class="flex items-center gap-10">
                <div role="progressbar" aria-label={label} aria-valuenow={percent} class="flex-1 h-8 r-pill bg-track overflow-hidden">
                    <div class="progress-fill" style={{ width: `${Math.max(3, percent)}%`, transition: 'width .4s' }} />
                </div>
                <span class="win-row-s c-text flex-none">{percent}%</span>
            </div>
            <span class="win-row-s">
                {label}
                {memoryMb !== undefined ? tr.voiceLab.jobMemory(fixed1(memoryMb / 1024)) : ''}
            </span>
        </div>
    );
}

/**
 * Одна модель голоса строкой выбора: в мастере «Свой голос» (EngineStep) и в выборе модели голоса персонажа на
 * компьютере-Турбо (EnginePicker). Ход скачивания — та же полоса JobBar; под строкой — свои кнопки.
 */
export function EngineRow({
    title,
    recommended,
    license,
    about,
    status,
    selected,
    disabled,
    onSelect,
    progress,
    children,
}: {
    title: string;
    recommended?: boolean;
    license?: string;
    about?: string;
    status?: string | null;
    selected: boolean;
    disabled?: boolean;
    onSelect: () => void;
    progress?: { label: string; percent: number };
    children?: ComponentChildren;
}) {
    return (
        <div
            role="radio"
            aria-checked={selected}
            aria-disabled={disabled}
            tabIndex={disabled ? -1 : 0}
            class={`win-row is-col win-choice${selected ? ' is-on' : ''}${disabled ? ' cursor-default opacity-55' : ''}`}
            onClick={() => !disabled && onSelect()}
            onKeyDown={(event) => (event.key === ' ' || event.key === 'Enter') && !disabled && onSelect()}
        >
            <div class="flex items-center gap-8 flex-wrap">
                <span class="win-row-t">{title}</span>
                {recommended && <span class="pill pill-agent">{tr.voiceLab.recommended}</span>}
                {license && <span class="win-chip">{license}</span>}
            </div>
            {about && <span class="win-row-s">{about}</span>}
            {status && <span class="win-row-s c-text">{status}</span>}
            {progress && <JobBar label={progress.label} percent={progress.percent} />}
            {children}
        </div>
    );
}

/**
 * Выбор модели голоса на Турбо: список — только когда подходят две и больше (заранее отмечена стоящая, иначе
 * советуемая); под ним место и ОЗУ выбранной — до кнопки, не после; Турбо спит — «поставится, когда проснётся».
 * Модель уже стоит на компьютере (владелец 18.11): предлагаем голос на ней — одной строкой, а список других — за
 * кнопкой «Выбрать другую»: ничего не качается, голос включается сразу.
 */
export function EnginePicker({ plan, engine, onSelect }: { plan: HomeVoicePlan | null | undefined; engine: string | null; onSelect: (engine: string) => void }) {
    const t = tr.wizard;
    const choices = usableEngines(plan);
    const picked = choices.find((choice) => choice.id === engine);
    const computer = plan?.plan?.computer ?? plan?.turbo?.name ?? '';
    const [open, setOpen] = useState(false);
    const offer = !open && choices.length > 1 && picked?.state === 'ready' ? picked : null;
    // Места мало — до кнопки: что поместится, а не поместится ничего — сколько освободить.
    const tight = choices.filter((choice) => !fitsDisk(choice));
    const fit = choices.find((choice) => choice.id === engine) ?? choices.find(fitsDisk);
    const free = tight[0]?.freeDiskMb ?? 0;
    const smallest = tight.reduce<VoicePlanEngine | null>((best, choice) => (!best || needMb(choice) < needMb(best) ? choice : best), null);
    const room =
        tight.length === 0
            ? null
            : fit
              ? t.diskSome(tight.map((choice) => choice.title), planGb(free), fit.title, fit.state === 'ready' ? null : planGb(needMb(fit)))
              : t.diskNone(planGb(free), smallest?.title ?? '', planGb(smallest ? needMb(smallest) : 0), planGb(Math.max(0, (smallest ? needMb(smallest) : 0) - free)));
    if (offer)
        return (
            <>
                <WinSection title={t.engineSection} group>
                    <WinRow title={t.engineOnReady(offer.title)} sub={t.engineWay[offer.way] || undefined}>
                        <button type="button" class="btn btn-plain btn-sm" onClick={() => setOpen(true)}>
                            {t.engineOther}
                        </button>
                    </WinRow>
                </WinSection>
                {offer.ramFits === false && <p class="win-callout m-0">{t.ramShort(planGb(offer.ramMb), planGb(offer.freeRamMb))}</p>}
                {plan?.turbo && !plan.turbo.up && <p class="win-note m-0">{t.turboAway(plan.turbo.name)}</p>}
            </>
        );
    return (
        <>
            {choices.length > 1 && (
                <WinSection title={t.engineSection}>
                    <div role="radiogroup" aria-label={t.engineSection} class="win-group">
                        {choices.map((choice) => (
                            <EngineRow
                                key={choice.id}
                                title={choice.title}
                                recommended={choice.recommended}
                                status={engineStatus(choice)}
                                selected={choice.id === engine}
                                disabled={!fitsDisk(choice)}
                                onSelect={() => onSelect(choice.id)}
                                progress={choice.state === 'installing' ? { label: t.stepModel(choice.title, computer), percent: choice.percent ?? 0 } : undefined}
                            />
                        ))}
                    </div>
                </WinSection>
            )}
            {room && <p class="win-callout m-0">{room}</p>}
            {picked && picked.ramFits === false && <p class="win-callout m-0">{t.ramShort(planGb(picked.ramMb), planGb(picked.freeRamMb))}</p>}
            {plan?.turbo && !plan.turbo.up && <p class="win-note m-0">{t.turboAway(plan.turbo.name)}</p>}
        </>
    );
}
