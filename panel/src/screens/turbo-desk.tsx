// Стол пульта Турбо (решение владельца 16.11, доска HelperDesk): компьютер только помогает — разговора здесь нет, орбиты
// тоже. В середине — его Турбо (что он даёт), шлюзы, которым помогает («ваш дом» / «чужой дом», что берут; колонка —
// ссылкой на её пульт), «Здесь не разговаривают» со ссылкой на пульт колонки и «Не давать компьютеру спать».

import { useState } from 'preact/hooks';

import { ApiError, api } from '../api';
import type { Ability, ClientView, PlacedStation } from '../api';
import { abilityTitle, useHelpers } from '../helpers';
import { formatList, tr } from '../i18n';
import { IconLaptop, IconSpeaker } from '../icons';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { flushPanelMemory } from '../shell/memory';
import { KeepAwake } from './helper-pc';
import type { TurboView } from './turbo-model';
import { TurboBody } from './turbo-scene';

/** Пульт колонки через проход этого компьютера: сначала память пульта, потом адрес. */
function openStation(key: string): void {
    void flushPanelMemory().then(() => location.assign(`${location.pathname}?via=${encodeURIComponent(key)}`));
}

const taken = (gives: Ability[]): string => formatList(gives.filter((id) => id === 'model' || id === 'hearing' || id === 'voice').map((id) => abilityTitle(id).toLowerCase()));

export function TurboDesk() {
    const panel = usePanel();
    const { view } = useHelpers();
    const [clients, setClients] = useState<ClientView[] | null>(null);
    usePoll({
        every: 30_000,
        version: panel.version,
        read: (signal) =>
            api
                .helperClients(signal)
                .then((answer) => !signal.aborted && setClients(answer.clients))
                .catch((e) => !signal.aborted && e instanceof ApiError && e.status === 404 && setClients(null)),
    });
    const placement = panel.health?.placement;
    const stations: PlacedStation[] = (placement?.stations ?? (placement?.station ? [placement.station] : [])).filter((entry) => entry.placement !== 'pc');
    const others = (clients ?? []).filter((client) => !client.station || !stations.some((entry) => entry.key === client.station));
    const helped = stations.filter((entry) => entry.placement === 'stationServer').length + others.length;
    const builtin = view?.helpers.find((helper) => helper.builtin);
    const turbo: TurboView | undefined = builtin && {
        helper: builtin,
        builtin: false,
        up: true,
        gives: builtin.abilities.filter((ability) => ability.ready).map((ability) => ability.id),
        abilities: builtin.abilities,
    };
    const first = stations[0];
    const firstName = first?.name ?? panel.role.station?.name;
    const home = () => panel.go('settings', 'helpers');
    return (
        <div class="turbo-desk">
            <div class="turbo-desk-main">
                {turbo && (
                    <TurboBody
                        turbo={turbo}
                        title={tr.dashboard.turboTitle(tr.core.tabThisComputer.toLowerCase())}
                        sub={helped ? tr.desk.helpsCount(helped) : tr.desk.helpsNobody}
                        state={tr.dashboard.turboWorks}
                        onOpen={home}
                    />
                )}
                <div class="turbo-desk-gateways">
                    {stations.map((entry) => {
                        const helping = entry.placement === 'stationServer';
                        const client = clients?.find((one) => one.station === entry.key);
                        const gives = client?.gives ?? (helping ? (['model', 'hearing', 'voice'] as Ability[]) : []);
                        return (
                            <div key={entry.key} class={`turbo-gw${entry.reachable ? '' : ' is-off'}`}>
                                <span class="turbo-gw-icon" aria-hidden="true">
                                    <IconSpeaker size={18} />
                                </span>
                                <span class="turbo-gw-text">
                                    <b>{entry.name ? tr.core.tabStation(entry.name) : tr.core.tabStationNoName}</b>
                                    <span>{[tr.desk.ownHome, helping ? gives.length > 0 && tr.desk.takes(taken(gives)) : tr.desk.worksAlone].filter(Boolean).join(' · ')}</span>
                                    <button class="truth-more" onClick={() => openStation(entry.key)}>
                                        {entry.name ? tr.desk.openStation(entry.name) : tr.desk.openStationNoName}
                                    </button>
                                </span>
                            </div>
                        );
                    })}
                    {others.map((client) => {
                        const names = client.gateways.map((gateway) => gateway.name).join(', ') || client.name;
                        const online = client.gateways.some((gateway) => gateway.online);
                        return (
                            <div key={client.id} class={`turbo-gw${online ? '' : ' is-off'}`}>
                                <span class="turbo-gw-icon" aria-hidden="true">
                                    <IconLaptop size={18} />
                                </span>
                                <span class="turbo-gw-text">
                                    <b>{names}</b>
                                    <span>{[client.ownHome ? tr.desk.ownHome : tr.desk.otherHome, client.gives.length > 0 && tr.desk.takes(taken(client.gives))].filter(Boolean).join(' · ')}</span>
                                </span>
                            </div>
                        );
                    })}
                </div>
            </div>
            <div class="turbo-desk-aside">
                <div class="turbo-desk-card">
                    <b>{tr.desk.noTalkTitle}</b>
                    <span class="w-cap">{tr.desk.noTalkAbout}</span>
                    {first && (
                        <button class="w-btn w-btn-ink self-start" onClick={() => openStation(first.key)}>
                            {firstName ? tr.desk.openStation(firstName) : tr.desk.openStationNoName}
                        </button>
                    )}
                </div>
                {/* Its own rule (helper-pc.tsx): in the desktop app only; out of it — nothing, and the card hides. */}
                <div class="turbo-desk-card turbo-desk-awake">
                    <KeepAwake />
                </div>
            </div>
        </div>
    );
}
