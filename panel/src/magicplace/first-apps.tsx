// Первый запуск (docs/plans/magicplace.md): после «Как будете говорить с DuoWave?» или подключения колонки — лист
// «Первые приложения»: что рекомендует каталог и что встанет здесь (сейчас — «Лента», только на компьютере). Каждое —
// одним нажатием, и словами, сколько оно скачает. Предложить нечего — листа нет; кто пропустил, увидит то же в ряду
// «Рекомендуем» MagicPlace.

import { useState } from 'preact/hooks';

import type { CatalogEntry, CatalogView } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { Action } from '../ui/cards';
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
    const actions = usePackageActions(() => undefined);
    const [done, setDone] = useState<string[]>([]);
    const install = async (entry: CatalogEntry) => {
        if (await actions.install(entry)) setDone((all) => [...all, entry.id]);
    };
    return (
        <Sheet
            title={tr.magicplace.firstTitle}
            about={tr.magicplace.firstAbout}
            icon={<MagicPlaceMark size={40} />}
            onClose={onClose}
            footer={
                <div class="mp-first-footer">
                    <button
                        class="dash-done"
                        onClick={() => {
                            onClose();
                            panel.go('magicplace');
                        }}
                    >
                        {tr.magicplace.firstAll}
                    </button>
                    <button class="dash-done" onClick={onClose}>
                        {done.length > 0 ? tr.magicplace.firstDone : tr.magicplace.firstLater}
                    </button>
                </div>
            }
        >
            {entries.map((entry) => {
                const installed = done.includes(entry.id);
                const busy = actions.busy[entry.id];
                return (
                    <div key={entry.id} class="pkg-row">
                        <div class="pkg-main">
                            <PackageThumb url={entry.preview?.image} />
                            <span class="pkg-text">
                                <span class="pkg-title">{entry.title}</span>
                                {entry.description && <span class="t-caption">{entry.description}</span>}
                                {downloadLines(entry).map((line) => (
                                    <span key={line} class="t-caption">
                                        {line}
                                    </span>
                                ))}
                            </span>
                            <div class="pkg-actions">
                                {installed ? (
                                    <span class="pill pill-own">{tr.magicplace.installed}</span>
                                ) : (
                                    <Action tone="on" label={tr.magicplace.install} busy={busy === 'install'} disabled={!!busy} onClick={() => void install(entry)} />
                                )}
                            </div>
                        </div>
                        <NoteLine note={actions.notes[entry.id]} />
                    </div>
                );
            })}
        </Sheet>
    );
}
