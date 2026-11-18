// Первые шаги (docs/plans/onboarding.md): как шаг говорит с рамкой. Рамка (screens/onboarding.tsx) рисует шапку, полосу
// шагов и кнопки «Назад» · «Дальше» — как на холсте «Онбординг». Шаг может переназвать «Дальше», запереть её, сделать
// своё по нажатию (подшаги: «Проверить» → «Подключить» внутри шага «Колонка») и уйти вперёд сам.

import { createContext } from 'preact';
import { useContext, useEffect } from 'preact/hooks';

/** Где будет жить DuoWave (второй шаг). */
export type Choice = 'computer' | 'stationPc' | 'station';

export interface StepNav {
    /** Надпись главной кнопки; без неё — «Дальше» (на последнем шаге — «На стол»). */
    label?: string;
    /** Кнопка заперта (ждём ответа, не выбрано нужное). */
    disabled?: boolean;
    /** Шаг занят (грузит то, что просили на нём): кнопка заперта и с крутилкой. */
    busy?: boolean;
    /**
     * Своё по нажатию. false — остаться на шаге (шаг сам сменил подэкран); иначе рамка идёт к следующему шагу.
     * Ошибка — рамка покажет её строкой над кнопками и останется.
     */
    onNext?: () => boolean | void | Promise<boolean | void>;
    /** Своё по «Назад» (подэкран назад). false — рамка остаётся на шаге. */
    onBack?: () => boolean | void;
    /** Спрятать «Назад». */
    hideBack?: boolean;
    /** Вторая кнопка слева от главной — белая капсула (например «Пропустить»). */
    secondary?: { label: string; onClick: () => void };
}

export interface Onboarding {
    choice: Choice;
    /** Слово колонки или компьютера: «Алиса» / «ДиВи» (en: Alice / Deevi). */
    wake: 'station' | 'computer';
    /** Перейти к следующему шагу (подключение кончилось — дальше само). */
    next: () => void;
    /** Закрыть первые шаги и открыть раздел пульта (MagicPlace и т. п.). */
    leave: (section?: string) => void;
    /** Перейти на шаг первых шагов (`speaker` — «Колонка»); такого шага на этом пути нет — ничего. */
    goTo?: (step: string) => void;
}

export const OnboardingContext = createContext<Onboarding>({ choice: 'computer', wake: 'computer', next: () => undefined, leave: () => undefined });
export const StepNavContext = createContext<(nav: StepNav | null) => void>(() => undefined);

/** Что знает шаг о первых шагах: выбор места, слово, переход дальше. */
export function useOnboarding(): Onboarding {
    return useContext(OnboardingContext);
}

/**
 * Шаг задаёт кнопки внизу. `deps` — когда пересчитать (как у useEffect); замыкания в `nav` берутся из последней
 * отрисовки с этими deps. Ушёл со шага — кнопки снова обычные.
 */
export function useStepNav(nav: StepNav, deps: unknown[]): void {
    const set = useContext(StepNavContext);
    useEffect(() => {
        set(nav);
    }, deps);
    useEffect(() => () => set(null), []);
}
