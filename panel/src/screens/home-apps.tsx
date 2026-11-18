// Один список «Установлено» дома (docs/plans/home-and-devices.md, этап 5а): у каждой карточки MagicPlace — где пакет
// будет работать в этом доме («работает: Кухня», «нужен компьютер-Турбо»), а в «Приложения → Установлено» под
// своими — что стоит у Турбо дома, по устройствам. Установка по связи — этап 5б: пока Турбо ставят у себя.

import { useState } from 'preact/hooks';

import { api, can, problemOf } from '../api';
import type { CatalogEntry, HomeApp, HomeAppPlace } from '../api';
import { useHomeDevices } from '../home-devices';
import { placeDevice } from '../home-model';
import { tr } from '../i18n';
import { placeOfEntry } from '../magicplace/filters';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';

/** Метка места на карточке пакета. */
export function PlacePill({ entry }: { entry: CatalogEntry }) {
    const { view } = useHomeDevices();
    // Приложение, что предпочитает дом, на колонке с Турбо работает у Турбо (этап 5б).
    const atHome = entry.preferHome && placeDevice('computer', view).device;
    const at = placeDevice(atHome ? 'computer' : placeOfEntry(entry), view);
    if (at.device) return <span class="pill pill-own">{tr.devices.runsOn(at.device.name)}</span>;
    if (at.missing) return <span class="pill pill-yandex">{at.missing === 'computer' ? tr.devices.needsComputer : tr.devices.needsStation}</span>;
    if (at.network) return <span class="pill pill-off">{tr.devices.inNetwork}</span>;
    return null;
}

/** «На других устройствах дома»: приложения Турбо по устройствам; кто спит — так и сказано. */
export function OtherDevicesApps() {
    const panel = usePanel();
    const { view } = useHomeDevices();
    const [answer, setAnswer] = useState<{ apps: HomeApp[]; places: HomeAppPlace[] } | null>(null);
    const [error, setError] = useState<string | null>(null);
    usePoll({
        on: can('station.view'),
        every: 60_000,
        version: panel.version,
        read: (signal) =>
            api.homeApps(signal).then(
                (next) => {
                    if (signal.aborted) return;
                    setAnswer(next);
                    setError(null);
                },
                (e) => !signal.aborted && setError(problemOf(e)),
            ),
    });
    const centre = view?.centre;
    const others = (answer?.places ?? []).filter((place) => place.device !== centre);
    if (others.length === 0) return error ? <p class="t-caption m-0 c-dot-offline">{error}</p> : null;
    return (
        <section class="card glass card-list" aria-labelledby="home-apps-h">
            <div class="section-head flex flex-col">
                <h2 id="home-apps-h" class="t-title-xs m-0">
                    {tr.devices.otherTitle}
                </h2>
                <span class="w-cap">{tr.devices.otherLead}</span>
            </div>
            {others.map((place) => {
                const apps = (answer?.apps ?? []).filter((app) => app.place.device === place.device && app.kind !== 'character' && app.kind !== 'theme');
                return (
                    <div key={place.device} class="flex flex-col gap-6">
                        <span class="w-strong flex items-center gap-6">
                            {place.name}
                            <span class="home-role is-turbo">{tr.devices.roleTurbo}</span>
                        </span>
                        {!place.reached ? (
                            <p class="w-cap m-0">{tr.devices.placeAway(place.name, place.why ?? tr.devices.stateOffline)}</p>
                        ) : (
                            <div class="w-rows">
                                {apps.map((app) => (
                                    <div key={`${place.device}:${app.id}`} class="w-row">
                                        <span class="flex-1 min-w-0 flex flex-col">
                                            <span class="w-strong">{app.title}</span>
                                            <span class="w-cap">
                                                {[app.version, app.enabled ? null : tr.catalog.stateOff].filter(Boolean).join(' · ')}
                                            </span>
                                        </span>
                                        <span class="pill pill-own">{tr.devices.runsOn(place.name)}</span>
                                    </div>
                                ))}
                            </div>
                        )}
                    </div>
                );
            })}
        </section>
    );
}
