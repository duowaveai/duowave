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
import { onComputer } from '../voice-input';
import { byMode } from '../words';
import { tr } from '../i18n';

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
    const s = tr.shell.search;
    return [
        { title: s.home.title, path: s.home.path, words: s.home.words, place: { section: 'home' } },
        { title: s.newWidget.title, path: s.newWidget.path, words: s.newWidget.words, act: 'add' },
        // «Сказать» в доке — микрофон пульта (docs/plans/voice-creation.md, решение 1); текст, который колонка произнесёт, — здесь.
        { title: s.sayText.title, path: s.sayText.path, words: s.sayText.words, act: 'say', when: () => can('station.say') && !onComputer() },

        { title: s.character.title, path: s.character.path, words: s.character.words, place: { section: 'character', tab: 'page' } },
        { title: s.allCharacters.title, path: s.allCharacters.path, words: s.allCharacters.words, place: { section: 'character', tab: 'characters' } },
        { title: s.voice.title, path: s.voice.path, words: s.voice.words, place: { section: 'character', tab: 'voice' } },
        { title: s.ownVoice.title, path: s.ownVoice.path, words: s.ownVoice.words, place: { section: 'character', tab: 'voice' } },
        { title: s.manner.title, path: s.manner.path, words: s.manner.words, place: { section: 'character', tab: 'page' } },
        { title: s.brain.title, path: s.brain.path, words: s.brain.words, place: { section: 'character', tab: 'brain' } },

        { title: s.memory.title, path: s.memory.path, words: s.memory.words, place: { section: 'skills', tab: 'memory', item: 'overview' }, when: () => can('memory.view') },
        { title: s.whatRemembers.title, path: s.whatRemembers.path, words: s.whatRemembers.words, place: { section: 'skills', tab: 'memory', item: 'overview' }, when: () => can('memory.view') },
        { title: s.house.title, path: s.house.path, words: s.house.words, place: { section: 'skills', tab: 'memory', item: 'house' }, when: () => can('memory.view') },
        { title: s.pause.title, path: s.pause.path, words: s.pause.words, place: { section: 'skills', tab: 'memory', item: 'overview' }, when: () => can('memory.view') },
        { title: s.memorySettings.title, path: s.memorySettings.path, words: s.memorySettings.words, place: { section: 'skills', tab: 'memory', item: 'settings' }, when: () => can('memory.view') },

        { title: s.apps.title, path: s.apps.path, words: s.apps.words, place: { section: 'skills', tab: 'list' }, when: () => can('station.view') },
        { title: s.installed.title, path: s.installed.path, words: s.installed.words, place: { section: 'skills', tab: 'installed' }, when: () => can('station.view') },
        { title: s.cantYet.title, path: s.cantYet.path, words: s.cantYet.words, place: { section: 'skills', tab: 'list' }, when: () => can('journal.view') },
        {
            title: s.connections.title, path: s.connections.path, words: s.connections.words,
            place: { section: 'skills', tab: 'connectors' },
            when: () => can('connectors.view'),
        },
        { title: s.mcp.title, path: s.mcp.path, words: s.mcp.words, place: { section: 'skills', tab: 'connectors' }, when: () => can('connectors.view') },
        { title: s.catalog.title, path: s.catalog.path, words: s.catalog.words, place: { section: 'skills', tab: 'catalog' }, when: () => can('station.view') },
        { title: s.workshop.title, path: s.workshop.path, words: s.workshop.words, place: { section: 'skills', tab: 'workshop' }, when: admin },
        { title: s.routes.title, path: s.routes.path, words: s.routes.words, place: { section: 'skills', tab: 'routes' }, when: () => !onComputer() },

        // Страница колонки в режиме «Микрофон» — страница микрофона (docs/plans/mic-mode.md): адрес тот же.
        byMode<SearchPlace>(
            { title: s.speaker.title, path: s.speaker.path, words: s.speaker.words, place: { section: 'settings', tab: 'station' } },
            { title: s.mic.title, path: s.mic.path, words: s.mic.words, place: { section: 'settings', tab: 'station' } },
        ),
        {
            title: s.howToTalk.title,
            path: byMode(s.howToTalk.path, s.howToTalk.pathMic),
            words: s.howToTalk.words,
            place: { section: 'settings', tab: 'station' },
            when: () => !helper && !onStation,
        },
        { title: s.mode.title, path: s.mode.path, words: s.mode.words, place: { section: 'settings', tab: 'station' }, when: () => !onComputer() },
        { title: s.hearing.title,
            path: byMode(s.hearing.path, s.hearing.pathMic),
            words: s.hearing.words, place: { section: 'settings', tab: 'station' } },
        {
            title: s.resources.title, path: s.resources.path, words: s.resources.words,
            place: { section: 'settings', tab: 'station' },
            when: () => onStation,
        },
        {
            title: s.placement.title, path: s.placement.path, words: s.placement.words,
            place: { section: 'settings', tab: 'station' },
            when: () => can('station.view') && !onComputer(),
        },
        {
            title: s.helper.title, path: s.helper.path, words: s.helper.words,
            place: { section: 'settings', tab: 'helper' },
            when: () => helper || onStation,
        },
        { title: s.access.title, path: s.access.path, words: s.access.words, place: { section: 'settings', tab: 'access' } },
        { title: s.look.title, path: s.look.path, words: s.look.words, place: { section: 'settings', tab: 'look' } },
        { title: s.darkLight.title, path: s.darkLight.path, words: s.darkLight.words, act: 'theme' },
        { title: s.updates.title, path: s.updates.path, words: s.updates.words, place: { section: 'settings', tab: 'updates' }, when: () => can('station.view') },
        { title: s.history.title, path: s.history.path, words: s.history.words, place: { section: 'settings', tab: 'history' }, when: () => can('journal.view') },
        { title: s.dev.title, path: s.dev.path, words: s.dev.words, place: { section: 'settings', tab: 'dev' } },
        { title: s.apiKeys.title, path: s.apiKeys.path, words: s.apiKeys.words, place: { section: 'settings', tab: 'access' }, when: () => can('users.manage') && devMode() },
        { title: s.models.title, path: s.models.path, words: s.models.words, place: { section: 'character', tab: 'brain' }, when: devMode },
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
            <button class="search-scrim" aria-label={tr.shell.closeSearch} tabIndex={-1} onClick={onClose} />
            <div ref={box} class="search-box glass-strong" role="dialog" aria-modal="true" aria-label={tr.shell.searchPanel}>
                <label class="search-field">
                    <IconSearch size={18} />
                    <input
                        type="search"
                        value={query}
                        placeholder={tr.shell.searchPlaceholder}
                        aria-label={tr.shell.searchWhat}
                        aria-controls="search-results"
                        aria-activedescendant={found[at] ? `search-hit-${at}` : undefined}
                        data-autofocus
                        onInput={(event) => setQuery((event.currentTarget as HTMLInputElement).value)}
                        onKeyDown={key}
                    />
                    <kbd class="search-kbd">esc</kbd>
                </label>
                <ul id="search-results" class="search-results" role="listbox" aria-label={tr.shell.searchFound}>
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
                    {found.length === 0 && <li class="search-none w-cap">{tr.shell.searchNone}</li>}
                </ul>
            </div>
        </div>
    );
}

/** ⌥Пробел в окне пульта — то же, что кнопка «Сказать» (docs/plans/mic-mode.md): начать слушать или закончить фразу. */
export function useTalkKey(talk: () => void): void {
    const latest = useRef(talk);
    latest.current = talk;
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.altKey && !event.metaKey && !event.ctrlKey && !event.shiftKey && event.code === 'Space' && !event.repeat) {
                event.preventDefault();
                latest.current();
            }
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, []);
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
