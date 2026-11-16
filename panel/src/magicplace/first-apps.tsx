// Первый запуск (docs/plans/magicplace.md): после «Как будете говорить с DuoWave?» или подключения колонки — лист
// «Первые приложения»: что рекомендует каталог и что встанет здесь (сейчас — «Лента», только на компьютере). Каждое —
// одним нажатием, и словами, сколько оно скачает. Предложить нечего — листа нет; кто пропустил, увидит то же в ряду
// «Рекомендуем» MagicPlace.

import { useState } from 'preact/hooks';

import type { CatalogEntry, CatalogView } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { Sheet } from '../ui/sheet';
import { NoteLine, PackageThumb, canManage, isInstalled, usePackageActions } from '../screens/catalog';
import { goesHere } from './filters';
import { MagicPlaceMark } from './icon';
import { downloadLines } from './section';

/** Что предложить в первом запуске: рекомендованное, что встанет здесь, ещё не стоит и что этот вход может поставить. */
export function firstAppsOf(view: CatalogView | null): CatalogEntry[] {
    if (!view || !view.configured || !view.enabled || view.offline) return [];
    return view.packages.filter((entry) => entry.recommended && !isInstalled(entry) && goesHere(entry) && canManage(entry.kind));
}

export function FirstAppsSheet({ entries, onClose }: { entries: CatalogEntry[]; onClose: () => void }) {
    const panel = usePanel();
    // Закрыть («Потом» / «Готово») — крестиком, Esc или мимо; внизу — только дорога в MagicPlace.
    return (
        <Sheet
            title={tr.magicplace.firstTitle}
            about={tr.magicplace.firstAbout}
            icon={<MagicPlaceMark size={40} />}
            onClose={onClose}
            footer={
                <button
                    class="btn btn-plain"
                    onClick={() => {
                        onClose();
                        panel.go('magicplace');
                    }}
                >
                    {tr.magicplace.firstAll}
                </button>
            }
        >
            <FirstAppsList entries={entries} />
        </Sheet>
    );
}

/** Рекомендованные приложения строками, каждое — «Установить» одним нажатием. */
export function FirstAppsList({ entries, onInstalled }: { entries: CatalogEntry[]; onInstalled?: (id: string) => void }) {
    const actions = usePackageActions(() => undefined);
    const [done, setDone] = useState<string[]>([]);
    const install = async (entry: CatalogEntry) => {
        if (!(await actions.install(entry))) return;
        setDone((all) => [...all, entry.id]);
        onInstalled?.(entry.id);
    };
    return (
        <div class="win-group">
            {entries.map((entry) => {
                const installed = done.includes(entry.id);
                const busy = actions.busy[entry.id];
                return (
                    <div key={entry.id} class="win-row is-col">
                        <div class="flex items-center gap-12">
                            <PackageThumb url={entry.preview?.image} />
                            <div class="win-row-main">
                                <span class="win-row-t">{entry.title}</span>
                                {entry.description && <span class="win-row-s">{entry.description}</span>}
                                {downloadLines(entry).map((line) => (
                                    <span key={line} class="win-row-s">
                                        {line}
                                    </span>
                                ))}
                            </div>
                            {installed ? (
                                <span class="pill pill-own">{tr.magicplace.installed}</span>
                            ) : (
                                <button class="btn btn-plain btn-sm" disabled={!!busy} aria-busy={busy === 'install' || undefined} onClick={() => void install(entry)}>
                                    {busy === 'install' && <span class="spinner spinner-sm" aria-hidden="true" />}
                                    {tr.magicplace.install}
                                </button>
                            )}
                        </div>
                        <NoteLine note={actions.notes[entry.id]} />
                    </div>
                );
            })}
        </div>
    );
}
