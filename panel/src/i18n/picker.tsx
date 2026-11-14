// Выбор языка пульта (docs/plans/i18n.md): переключатель на экране первого запуска и входа и карточка «Язык» в
// «Настройках» → «Оформление». Языков немного — кнопки рядом, как режим темы; больше четырёх — список.

import { useEffect, useState } from 'preact/hooks';

import { api, can, problemOf } from '../api';
import { LANGUAGES, languageInfo, setLanguage, tr, useLanguage } from '.';

/** Языки кнопками или списком: самоназвания, выбранный отмечен. */
export function LanguageSwitch({ onPick, class: className = '' }: { onPick?: (code: string) => void; class?: string }) {
    const current = useLanguage();
    const choose = (code: string): void => {
        void setLanguage(code).then(() => onPick?.(code));
    };
    if (LANGUAGES.length > 4) {
        return (
            <select class={`field ${className}`} aria-label={tr.settings.languageLabel} value={current} onChange={(event) => choose((event.target as HTMLSelectElement).value)}>
                {LANGUAGES.map((language) => (
                    <option key={language.code} value={language.code} lang={language.code}>
                        {language.name}
                    </option>
                ))}
            </select>
        );
    }
    return (
        <div
            role="radiogroup"
            aria-label={tr.settings.languageLabel}
            class={`look-seg glass ${className}`}
            style={{ gridTemplateColumns: `repeat(${LANGUAGES.length}, minmax(0, 1fr))` }}
        >
            {LANGUAGES.map((language) => (
                <button key={language.code} role="radio" lang={language.code} aria-checked={current === language.code} onClick={() => choose(language.code)}>
                    {language.name}
                </button>
            ))}
        </div>
    );
}

/**
 * «Язык» в «Настройках»: язык пульта на этом устройстве и язык дома — на нём говорят приложения, расписания и
 * уведомления. У кого есть право менять настройки, тот выбором меняет оба.
 */
export function LanguageCard() {
    const current = useLanguage();
    const mayEdit = can('settings.edit');
    const [household, setHousehold] = useState<string | null | undefined>(undefined);
    const [problem, setProblem] = useState<string | null>(null);
    useEffect(() => {
        let alive = true;
        api.language().then(
            (answer) => alive && setHousehold(answer.language),
            () => alive && setHousehold(undefined),
        );
        return () => {
            alive = false;
        };
    }, []);
    const picked = async (code: string): Promise<void> => {
        setProblem(null);
        if (!mayEdit || household === code) return;
        try {
            setHousehold((await api.setLanguage(code)).language);
        } catch (error) {
            setProblem(problemOf(error));
        }
    };
    const householdName = household ? (languageInfo(household)?.name ?? household) : null;
    return (
        <section class="card glass card-list" aria-labelledby="language-h">
            <div class="section-head">
                <h2 id="language-h" class="t-title-xs">
                    {tr.settings.languageTitle}
                </h2>
                <span class="t-meta">{tr.settings.languageWhere}</span>
            </div>
            <LanguageSwitch onPick={(code) => void picked(code)} />
            <p class="t-caption m-0">
                {householdName && household !== current
                    ? tr.settings.householdDiffers(householdName)
                    : mayEdit
                      ? tr.settings.householdFollows
                      : tr.settings.householdOnlyAdmin}
            </p>
            {mayEdit && household && household !== current && (
                <div>
                    <button class="pill pill-own pill-btn" onClick={() => void picked(current)}>
                        {tr.settings.makeHousehold(languageInfo(current)?.name ?? current)}
                    </button>
                </div>
            )}
            {problem && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
        </section>
    );
}
