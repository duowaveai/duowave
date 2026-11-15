// Страна дома (docs/plans/catalog-regions.md, 3): вопрос на столе, пока страна не выбрана (первый запуск и старые
// установки — одной плашкой), и карточка в «Настройках» рядом с языком. Догадка — country.ts: часовой пояс и язык
// этого компьютера, никогда не адрес. Отвечает тот, кто меняет настройки; остальным вопрос не показывается.

import { useEffect, useMemo, useState } from 'preact/hooks';

import { api, can, problemOf } from '../api';
import { countryList, countryName, flagOf, guessCountries } from '../country';
import { tr, useLanguage } from '../i18n';

/** Ответ шлюза: код, `none`, null — не спрошено; undefined — ещё читаем или шлюз постарше. */
type Answer = string | null | undefined;

const listeners = new Set<(answer: Answer) => void>();
let known: Answer = undefined;

function tell(answer: Answer): void {
    known = answer;
    for (const listener of listeners) listener(answer);
}

/** Страна дома, общая для вопроса и карточки: ответили в одном месте — второе видит сразу. */
export function useHomeCountry(): [Answer, (code: string) => Promise<void>] {
    const [answer, setAnswer] = useState<Answer>(known);
    useEffect(() => {
        listeners.add(setAnswer);
        if (known === undefined)
            api.homeCountry().then(
                (view) => tell(view.country),
                () => undefined,
            );
        return () => {
            listeners.delete(setAnswer);
        };
    }, []);
    const choose = async (code: string): Promise<void> => {
        tell((await api.setHomeCountry(code)).country);
    };
    return [answer, choose];
}

function label(code: string, locale: string): string {
    return `${flagOf(code)} ${countryName(code, locale)}`.trim();
}

/** Список всех стран с поиском: догадки первыми. */
function CountryPicker({ onPick, guesses }: { onPick: (code: string) => void; guesses: string[] }) {
    const locale = useLanguage();
    const [query, setQuery] = useState('');
    const all = useMemo(() => countryList(locale), [locale]);
    const needle = query.trim().toLocaleLowerCase(locale);
    const shown = needle ? all.filter((entry) => entry.name.toLocaleLowerCase(locale).includes(needle) || entry.code.toLowerCase() === needle) : all;
    const ordered = needle ? shown : [...guesses.map((code) => ({ code, name: countryName(code, locale) })), ...shown.filter((entry) => !guesses.includes(entry.code))];
    return (
        <div class="grid gap-8">
            <input class="field" type="search" placeholder={tr.country.search} aria-label={tr.country.search} value={query} onInput={(event) => setQuery((event.target as HTMLInputElement).value)} />
            <select
                class="field"
                size={6}
                aria-label={tr.country.pick}
                onChange={(event) => {
                    const code = (event.target as HTMLSelectElement).value;
                    if (code) onPick(code);
                }}
            >
                {ordered.map((entry) => (
                    <option key={entry.code} value={entry.code}>
                        {label(entry.code, locale)}
                    </option>
                ))}
            </select>
        </div>
    );
}

/** Плашка на столе: страна не выбрана, а отвечающий может её выбрать. */
export function CountryAsk() {
    const [answer, choose] = useHomeCountry();
    const locale = useLanguage();
    const [picking, setPicking] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const guesses = useMemo(() => guessCountries(), []);
    if (answer !== null || !can('settings.edit')) return null;
    const pick = (code: string): void => {
        setProblem(null);
        choose(code).catch((error) => setProblem(problemOf(error)));
    };
    return (
        <section class="card glass first-run" aria-labelledby="country-ask-h">
            <h2 id="country-ask-h" class="t-title m-0">
                {tr.country.askTitle}
            </h2>
            <p class="t-caption m-0">{tr.country.askAbout}</p>
            <div class="flex flex-wrap gap-8">
                {guesses.slice(0, 2).map((code, index) => (
                    <button key={code} class={index === 0 ? 'w-btn w-btn-ink' : 'w-btn'} onClick={() => pick(code)}>
                        {label(code, locale)}
                    </button>
                ))}
                <button class="w-btn" onClick={() => setPicking(!picking)} aria-expanded={picking}>
                    {tr.country.other}
                </button>
                <button class="w-btn" onClick={() => pick('none')}>
                    {tr.country.wontSay}
                </button>
            </div>
            {picking && <CountryPicker guesses={guesses} onPick={pick} />}
            {problem && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
        </section>
    );
}

/** «Страна дома» в «Настройках»: что выбрано и смена. */
export function CountryCard() {
    const [answer, choose] = useHomeCountry();
    const locale = useLanguage();
    const mayEdit = can('settings.edit');
    const [picking, setPicking] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const guesses = useMemo(() => guessCountries(), []);
    if (answer === undefined) return null;
    const pick = (code: string): void => {
        setProblem(null);
        choose(code).then(
            () => setPicking(false),
            (error) => setProblem(problemOf(error)),
        );
    };
    const now = answer === null ? tr.country.notAsked : answer === 'none' ? tr.country.none : label(answer, locale);
    return (
        <section class="card glass card-list" aria-labelledby="country-h">
            <div class="section-head">
                <h2 id="country-h" class="t-title-xs">
                    {tr.country.cardTitle}
                </h2>
                <span class="t-meta">{tr.country.cardWhere}</span>
            </div>
            <div class="flex flex-wrap items-center gap-8">
                <span class="t-body">{now}</span>
                {mayEdit && (
                    <button class="pill pill-own pill-btn" onClick={() => setPicking(!picking)} aria-expanded={picking}>
                        {tr.country.other}
                    </button>
                )}
                {mayEdit && answer !== 'none' && (
                    <button class="pill pill-btn" onClick={() => pick('none')}>
                        {tr.country.wontSay}
                    </button>
                )}
            </div>
            {picking && <CountryPicker guesses={guesses} onPick={pick} />}
            <p class="t-caption m-0">{mayEdit ? tr.country.cardAbout : tr.country.onlyAdmin}</p>
            {problem && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
        </section>
    );
}
