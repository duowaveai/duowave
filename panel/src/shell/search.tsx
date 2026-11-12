// Поиск по пульту (docs/plans/panel-rebuild.md, «Три слоя»): ⌘K или Ctrl+K, кнопка-лупа в верхней строке. Ищет места
// пульта по имени и по словам, которыми их называют люди («тема», «журнал», «telegram», «распознавание», «ключи»), и
// ведёт туда; находит и то, что лежит глубоко, и слой разработчика, когда тот включён. Стрелки выбирают, Enter
// открывает, Esc закрывает.

import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import { can, sessionStore } from '../api';
import { IconSearch } from '../icons';
import { useDialogFocus } from '../ui/sheet';
import { devMode } from './dev';
import type { Place } from './links';

export interface SearchPlace {
    title: string;
    /** Где это: «Настройки → Колонка». Пусто — само место дока. */
    path: string;
    /** Как это ещё называют — по ним тоже находится. */
    words: string;
    /** Куда вести; нет — действие (`act`). */
    place?: Omit<Place, 'query'> & { query?: Record<string, string> };
    act?: 'say' | 'add' | 'theme';
    /** Кому видно: права шлюза, слой разработчика, роль. */
    when?: () => boolean;
}

const admin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Места пульта. Новое место — строка здесь: тогда его найдут. */
export function searchPlaces(helper: boolean, onStation: boolean): SearchPlace[] {
    return [
        { title: 'Главная', path: '', words: 'стол колонка виджеты дом главный экран', place: { section: 'home' } },
        { title: 'Новый виджет', path: 'Главная', words: 'добавить виджет плитка плюс', act: 'add' },
        // «Сказать» в доке — микрофон пульта (docs/plans/voice-creation.md, решение 1); текст, который колонка произнесёт, — здесь.
        { title: 'Текстом колонке…', path: 'Главная', words: 'сказать через колонку произнести озвучить текст колонке', act: 'say', when: () => can('station.say') },

        { title: 'Персонаж', path: '', words: 'персонажи кто отвечает алиса артур мила свой персонаж', place: { section: 'character', tab: 'page' } },
        { title: 'Все персонажи', path: 'Персонаж', words: 'выбрать другого персонажа каталог персонажей', place: { section: 'character', tab: 'characters' } },
        { title: 'Голос', path: 'Персонаж', words: 'голос озвучка tts silero piper бая ксения айдар голоса', place: { section: 'character', tab: 'voice' } },
        { title: 'Свой голос', path: 'Персонаж → Голос', words: 'клон записать свой голос лаборатория минута', place: { section: 'character', tab: 'voice' } },
        { title: 'Манера', path: 'Персонаж', words: 'характер манера длина ответа шутки промпт как отвечает', place: { section: 'character', tab: 'page' } },
        { title: 'Мозг', path: 'Персонаж', words: 'мозг модель llm claude qwen gpt deepseek мышление думать дольше', place: { section: 'character', tab: 'brain' } },

        { title: 'Память', path: 'Приложения', words: 'память что помнит запомнить запомни забыть забудь факты', place: { section: 'skills', tab: 'memory', item: 'overview' }, when: () => can('memory.view') },
        { title: 'Что помнит персонаж', path: 'Приложения → Память', words: 'глазами персонажа что знает помнит подсказка', place: { section: 'skills', tab: 'memory', item: 'overview' }, when: () => can('memory.view') },
        { title: 'Люди и вещи дома', path: 'Приложения → Память', words: 'люди питомцы комнаты кто где кто с кем связи семья', place: { section: 'skills', tab: 'memory', item: 'house' }, when: () => can('memory.view') },
        { title: 'Не запоминать: пауза', path: 'Приложения → Память', words: 'пауза не запоминай не запоминать', place: { section: 'skills', tab: 'memory', item: 'overview' }, when: () => can('memory.view') },
        { title: 'Настройки памяти', path: 'Приложения → Память', words: 'запоминать мимоходом выключить память сон рефлексия', place: { section: 'skills', tab: 'memory', item: 'settings' }, when: () => can('memory.view') },

        { title: 'Приложения', path: '', words: 'приложения навыки плагины пакеты что умеет колонка возможности', place: { section: 'skills', tab: 'list' }, when: () => can('station.view') },
        { title: 'Установленные приложения', path: 'Приложения', words: 'установленные навыки плагины пакеты установлено включить выключить обновить', place: { section: 'skills', tab: 'installed' }, when: () => can('station.view') },
        { title: 'Пока не умеем', path: 'Приложения', words: 'научить не умеет не понял навык сделать', place: { section: 'skills', tab: 'list' }, when: () => can('journal.view') },
        {
            title: 'Подключения',
            path: 'Приложения',
            words: 'коннекторы подключения telegram телеграм умный дом home assistant звонки новости погода',
            place: { section: 'skills', tab: 'connectors' },
            when: () => can('connectors.view'),
        },
        { title: 'Программы (MCP)', path: 'Приложения → Подключения', words: 'mcp программы инструменты агент сервер', place: { section: 'skills', tab: 'connectors' }, when: () => can('connectors.view') },
        { title: 'Каталог', path: 'Приложения', words: 'каталог магазин добавить навык приложение плагин установить', place: { section: 'skills', tab: 'catalog' }, when: () => can('station.view') },
        { title: 'Мастерская', path: 'Приложения', words: 'мастерская сделать своё приложение свой навык плагин код виджет агент черновик', place: { section: 'skills', tab: 'workshop' }, when: admin },
        { title: 'Кто отвечает', path: 'Приложения', words: 'маршруты кто отвечает свой яндекс таблица', place: { section: 'skills', tab: 'routes' } },

        { title: 'Колонка', path: 'Настройки', words: 'колонка имя вид станция лайт колонки', place: { section: 'settings', tab: 'station' } },
        { title: 'Режим', path: 'Настройки → Колонка', words: 'режим яндекс вместе своё шлюз', place: { section: 'settings', tab: 'station' } },
        { title: 'Слух', path: 'Настройки → Колонка', words: 'слух распознавание asr whisper gigaam речь', place: { section: 'settings', tab: 'station' } },
        {
            title: 'Ресурсы колонки',
            path: 'Настройки → Колонка',
            words: 'ресурсы оперативная озу место диск плагины давление',
            place: { section: 'settings', tab: 'station' },
            when: () => onStation,
        },
        {
            title: 'Где работает DuoWave',
            path: 'Настройки → Колонка',
            words: 'размещение где работает шлюз на колонке на компьютере перенести вернуть',
            place: { section: 'settings', tab: 'station' },
            when: () => can('station.view'),
        },
        {
            title: 'Компьютер-помощник',
            path: 'Настройки',
            words: 'помощник компьютер помогать колонке не засыпать модели на компьютере',
            place: { section: 'settings', tab: 'helper' },
            when: () => helper || onStation,
        },
        { title: 'Люди и доступ', path: 'Настройки', words: 'люди доступ пароль вход пользователи роли гость', place: { section: 'settings', tab: 'access' } },
        { title: 'Оформление', path: 'Настройки', words: 'тема оформление тёмная светлая вид боковая панель цвет', place: { section: 'settings', tab: 'look' } },
        { title: 'Тёмная или светлая тема', path: 'Оформление', words: 'тема тёмная светлая ночь день', act: 'theme' },
        { title: 'Обновления и копии', path: 'Настройки', words: 'обновление версия копия страховка резерв', place: { section: 'settings', tab: 'updates' }, when: () => can('station.view') },
        { title: 'История разговоров', path: 'Настройки', words: 'история журнал вопросы разговоры что спрашивали', place: { section: 'settings', tab: 'history' }, when: () => can('journal.view') },
        { title: 'Для разработчиков', path: 'Настройки', words: 'разработчик env провайдер ключ api журнал запросов слой', place: { section: 'settings', tab: 'dev' } },
        { title: 'Ключи API', path: 'Настройки → Люди и доступ', words: 'ключи api токен home assistant', place: { section: 'settings', tab: 'access' }, when: () => can('users.manage') && devMode() },
        { title: 'Модели и провайдеры', path: 'Персонаж → Мозг', words: 'провайдер env ключ модели ollama openai anthropic', place: { section: 'character', tab: 'brain' }, when: devMode },
    ];
}

/** Насколько место подходит под запрос: 0 — не подходит; больше — выше в списке. */
function score(entry: SearchPlace, query: string): number {
    const q = query.trim().toLowerCase().replace(/ё/g, 'е');
    if (!q) return 1;
    const norm = (text: string) => text.toLowerCase().replace(/ё/g, 'е');
    const title = norm(entry.title);
    if (title.startsWith(q)) return 4;
    if (title.includes(q)) return 3;
    if (norm(entry.words).split(/\s+/).some((word) => word.startsWith(q))) return 2;
    return norm(`${entry.path} ${entry.words}`).includes(q) ? 1 : 0;
}

export function SearchPalette({
    places,
    onPick,
    onClose,
}: {
    places: SearchPlace[];
    onPick: (entry: SearchPlace) => void;
    onClose: () => void;
}) {
    const [query, setQuery] = useState('');
    const [at, setAt] = useState(0);
    const box = useRef<HTMLDivElement>(null);
    useDialogFocus(box, onClose);
    const found = useMemo(
        () =>
            places
                .filter((entry) => !entry.when || entry.when())
                .map((entry) => ({ entry, rank: score(entry, query) }))
                .filter((hit) => hit.rank > 0)
                .sort((a, b) => b.rank - a.rank)
                .slice(0, 9)
                .map((hit) => hit.entry),
        [places, query],
    );
    useEffect(() => setAt(0), [query]);
    const key = (event: KeyboardEvent) => {
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            if (found.length) setAt((at + (event.key === 'ArrowDown' ? 1 : -1) + found.length) % found.length);
        } else if (event.key === 'Enter' && found[at]) {
            event.preventDefault();
            onPick(found[at]);
        }
    };
    return (
        <div class="search-layer">
            <button class="search-scrim" aria-label="Закрыть поиск" tabIndex={-1} onClick={onClose} />
            <div ref={box} class="search-box glass-strong" role="dialog" aria-modal="true" aria-label="Поиск по пульту">
                <label class="search-field">
                    <IconSearch size={18} />
                    <input
                        type="search"
                        value={query}
                        placeholder="Найти: голос, тема, Telegram, история…"
                        aria-label="Что найти"
                        aria-controls="search-results"
                        aria-activedescendant={found[at] ? `search-hit-${at}` : undefined}
                        data-autofocus
                        onInput={(event) => setQuery((event.currentTarget as HTMLInputElement).value)}
                        onKeyDown={key}
                    />
                    <kbd class="search-kbd">esc</kbd>
                </label>
                <ul id="search-results" class="search-results" role="listbox" aria-label="Найдено">
                    {found.map((entry, index) => (
                        <li
                            key={`${entry.title}-${entry.path}`}
                            id={`search-hit-${index}`}
                            role="option"
                            aria-selected={index === at}
                            class={`search-hit${index === at ? ' on' : ''}`}
                            onMouseEnter={() => setAt(index)}
                            onClick={() => onPick(entry)}
                        >
                            <b>{entry.title}</b>
                            {entry.path && <span class="w-cap">{entry.path}</span>}
                        </li>
                    ))}
                    {found.length === 0 && <li class="search-none w-cap">Ничего не нашлось. Попробуйте другое слово.</li>}
                </ul>
            </div>
        </div>
    );
}

/** ⌘K или Ctrl+K в любом месте пульта — открыть поиск. */
export function useSearchKey(open: () => void): void {
    const latest = useRef(open);
    latest.current = open;
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'k') {
                event.preventDefault();
                latest.current();
            }
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, []);
}
