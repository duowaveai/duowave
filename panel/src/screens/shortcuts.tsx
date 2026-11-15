// Ярлыки на столе (docs/plans/menu-scheme.md): значок и имя, нажатие открывает приложение — его окно, а без окна —
// его страницу в «Приложениях». Только открывают: статусы — у виджетов. Сразу стоят MagicPlace и «Папки» (владелец,
// 15.11); «+ Ярлык» — список приложений с галочками, голосом — «добавь ярлык …», «убери ярлык …». Список живёт в
// раскладке стола (shell/widgets.ts), раскладка — на шлюзе: ярлыки одни на всех устройствах человека. На широком столе
// — столбиком у левого края, на месте, когда стол двигают; в ленте — рядом сверху. Некоторые приложения
// (`SHORTCUT_WHEN_INSTALLED`, AirPlay) ставят свой ярлык сами, когда их поставили, — один раз.

import { useEffect, useState } from 'preact/hooks';

import { PackageView, api, can } from '../api';
import { tr } from '../i18n';
import { IconFolder, IconPlus } from '../icons';
import { UpdatesBadge } from '../magicplace/widget';
import { useStoreView } from '../magicplace/state';
import { usePanel } from '../shell/context';
import type { Icon } from '../shell/sections';
import { DeskActions, DeskLayout, SHORTCUT_WHEN_INSTALLED, STARTING_SHORTCUTS } from '../shell/widgets';
import { AppIcon } from '../ui/app-icon';
import { useVoiceAction } from '../voice/phrases';
import { appSectionOf } from './plugins';

/** Что может стоять ярлыком: встроенное (магазин, «Папки») и включённые приложения. */
interface Candidate {
    id: string;
    title: string;
    icon?: Icon;
    image?: string | null;
    open: () => void;
}

/** Включённые приложения дома — для ярлыков; без права их видеть — только встроенное. */
function useApps(): PackageView[] | null {
    const panel = usePanel();
    const [apps, setApps] = useState<PackageView[] | null>(null);
    useEffect(() => {
        if (!can('connectors.view')) return setApps([]);
        let alive = true;
        api.packages()
            .then((view) => alive && setApps(view.packages.filter((entry) => entry.kind !== 'character' && entry.kind !== 'theme' && entry.enabled)))
            .catch(() => alive && setApps([]));
        return () => {
            alive = false;
        };
    }, [panel.version]);
    return apps;
}

export function useShortcutCandidates(): Candidate[] | null {
    const panel = usePanel();
    const apps = useApps();
    const store = useStoreView();
    if (!apps) return null;
    const pictures = new Map((store?.packages ?? []).map((entry) => [entry.id, entry.preview?.image]));
    const builtIn: Candidate[] = can('station.view')
        ? [
              { id: 'magicplace', title: tr.magicplace.title, open: () => panel.go('magicplace') },
              { id: 'folders', title: tr.core.sectionFolders, icon: IconFolder, open: () => panel.go('folders') },
          ]
        : [];
    const own = apps
        .filter((entry) => !builtIn.some((known) => known.id === entry.id))
        .map((entry): Candidate => {
            const section = appSectionOf(entry.id);
            return {
                id: entry.id,
                title: entry.title,
                image: pictures.get(entry.id),
                open: () => (section ? panel.go(section) : panel.go('skills', 'installed', entry.id)),
            };
        });
    return [...builtIn, ...own];
}

/** Похоже ли сказанное на имя: «календаря» — «Календарь». Падежи срезаются концом слова. */
function sounds(heard: string, title: string): boolean {
    const said = heard.trim().toLowerCase();
    const name = title.trim().toLowerCase();
    if (!said || !name) return false;
    const stem = name.slice(0, Math.max(3, name.length - 2));
    return said.startsWith(stem) || name.startsWith(said);
}

export function Shortcuts({ layout, actions, place }: { layout: DeskLayout; actions: DeskActions; place: 'column' | 'row' }) {
    const candidates = useShortcutCandidates();
    const [menu, setMenu] = useState(false);
    const list = layout.shortcuts ?? STARTING_SHORTCUTS;
    const shown = (candidates ?? []).filter((entry) => list.includes(entry.id)).sort((a, b) => list.indexOf(a.id) - list.indexOf(b.id));
    const toggle = (id: string, on: boolean) => actions.shortcuts(on ? [...list.filter((entry) => entry !== id), id] : list.filter((entry) => entry !== id));
    // Голосом: «добавь ярлык календаря», «убери ярлык радио».
    const byVoice = (rest: string, on: boolean) => {
        const found = candidates?.find((entry) => sounds(rest, entry.title));
        if (found) toggle(found.id, on);
    };
    useVoiceAction(candidates ? [tr.desk.shortcutAddPhrase] : [], (rest) => byVoice(rest, true), false);
    useVoiceAction(candidates ? [tr.desk.shortcutRemovePhrase] : [], (rest) => byVoice(rest, false), false);
    // Поставили AirPlay — его ярлык встаёт на стол сам, один раз; убрали — не возвращается.
    const offered = layout.offered ?? [];
    const due = (candidates ?? []).filter((entry) => SHORTCUT_WHEN_INSTALLED.includes(entry.id) && !offered.includes(entry.id));
    useEffect(() => {
        for (const entry of due) actions.offer(entry.id);
    }, [due.map((entry) => entry.id).join(',')]);
    useEffect(() => {
        if (!menu) return;
        const onKey = (event: KeyboardEvent) => event.key === 'Escape' && setMenu(false);
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [menu]);
    if (!candidates) return null;
    return (
        <nav class={`desk-shortcuts is-${place}`} aria-label={tr.desk.shortcuts}>
            {shown.map((entry) => (
                <button key={entry.id} class="desk-shortcut" onClick={entry.open}>
                    <span class="desk-shortcut-icon">
                        <AppIcon id={entry.id} title={entry.title} icon={entry.icon} image={entry.image} size={52} />
                        {entry.id === 'magicplace' && <UpdatesBadge />}
                    </span>
                    <span class="desk-shortcut-name">{entry.title}</span>
                </button>
            ))}
            <div class="desk-shortcut-add-wrap">
                <button class="desk-shortcut is-add" aria-expanded={menu} aria-haspopup="dialog" onClick={() => setMenu((open) => !open)}>
                    <span class="desk-shortcut-icon">
                        <span class="desk-shortcut-plus">
                            <IconPlus size={22} />
                        </span>
                    </span>
                    <span class="desk-shortcut-name">{tr.desk.shortcutAdd}</span>
                </button>
                {menu && (
                    <>
                        <button class="desk-shortcut-scrim" aria-label={tr.desk.shortcutsClose} onClick={() => setMenu(false)} />
                        <div class="desk-shortcut-menu menu" role="dialog" aria-label={tr.desk.shortcutsMenu}>
                            <span class="desk-shortcut-menu-title">{tr.desk.shortcutsMenu}</span>
                            {candidates.map((entry) => (
                                <label key={entry.id} class="desk-shortcut-option">
                                    <input type="checkbox" checked={list.includes(entry.id)} onChange={(event) => toggle(entry.id, (event.target as HTMLInputElement).checked)} />
                                    <span>{entry.title}</span>
                                </label>
                            ))}
                            <span class="desk-shortcut-menu-hint">{tr.desk.shortcutsVoice}</span>
                        </div>
                    </>
                )}
            </div>
        </nav>
    );
}
