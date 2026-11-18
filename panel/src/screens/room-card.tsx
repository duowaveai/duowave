// «Комната» на странице колонки (docs/plans/home-and-devices.md, «Комнаты», этап 2): по комнатам отличаются только
// режим Алисы и кто отвечает здесь; память, люди и приложения — общие для дома. Хранит центр
// (`/api/v1/rooms`); «Как во всём доме» снимает своё у комнаты.

import { useState } from 'preact/hooks';

import { api, can, problemOf } from '../api';
import type { CharactersView, HomeRoom, PanelMode } from '../api';
import { tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';

const MODES: PanelMode[] = ['relay', 'gateway', 'own'];
const modeLabel = (mode: PanelMode): string => ({ relay: tr.home.modeRelay, gateway: tr.home.modeGateway, own: tr.home.modeOwn })[mode];

export function RoomCard({ device, centre }: { device: string; centre: boolean }) {
    const panel = usePanel();
    const [room, setRoom] = useState<HomeRoom | null>(null);
    const [characters, setCharacters] = useState<CharactersView | null>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState(false);
    usePoll({
        on: can('station.view'),
        version: panel.version,
        read: (signal) =>
            Promise.all([api.homeRooms(signal), api.characters()]).then(
                ([rooms, list]) => {
                    if (signal.aborted) return;
                    setRoom(rooms.rooms[device] ?? {});
                    setCharacters(list);
                },
                (e) => !signal.aborted && setNote({ ok: false, text: problemOf(e) }),
            ),
    });
    const edit = can('settings.edit');
    const save = async (change: Partial<Record<keyof HomeRoom, unknown>>, reconnects = false): Promise<void> => {
        setBusy(true);
        setNote(null);
        try {
            const answer = await api.setHomeRoom(device, change);
            setRoom({ mode: answer.mode, volume: answer.volume, character: answer.character });
            if (reconnects) setNote({ ok: true, text: tr.devices.roomSaved });
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const activeId = characters?.active?.id;
    const home = characters?.characters.find((character) => character.id === activeId)?.title;
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`room-${device}`}>
            <div class="section-head flex flex-col">
                <h2 id={`room-${device}`} class="t-title-xs m-0">
                    {tr.devices.roomWho}
                </h2>
                <span class="w-cap">{tr.devices.roomWhoAbout}</span>
            </div>
            <select
                class="field"
                aria-label={tr.devices.roomWho}
                disabled={!edit || busy || !room}
                value={room?.character ?? ''}
                onChange={(event) => {
                    const value = (event.target as HTMLSelectElement).value;
                    void save({ character: value || null });
                }}
            >
                <option value="">{home ? tr.devices.roomWhoHome(home) : tr.devices.roomWhoHomePlain}</option>
                {(characters?.characters ?? []).map((character) => (
                    <option key={character.id} value={character.id}>
                        {character.title}
                    </option>
                ))}
            </select>
            {!centre && (
                <div class="flex flex-col gap-6">
                    <span class="w-strong">{tr.devices.roomMode}</span>
                    <div class="seg" role="radiogroup" aria-label={tr.devices.roomMode}>
                        <button type="button" role="radio" aria-checked={!room?.mode} class="seg-btn flex-1" disabled={!edit || busy} onClick={() => void save({ mode: null }, true)}>
                            {tr.devices.roomModeHome}
                        </button>
                        {MODES.map((mode) => (
                            <button key={mode} type="button" role="radio" aria-checked={room?.mode === mode} class="seg-btn flex-1" disabled={!edit || busy} onClick={() => void save({ mode }, true)}>
                                {modeLabel(mode)}
                            </button>
                        ))}
                    </div>
                </div>
            )}
            {note && (
                <p class={`w-cap m-0${note.ok ? '' : ' c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </p>
            )}
        </section>
    );
}
