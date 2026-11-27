// The question's path the dashboard follows (screens/dashboard.tsx, the orbit), without the DOM so that
// scripts/panel-check.ts checks it.

import type { RequestView } from '../api';

/** The device of a computer's own ear, as its trips name it (the gateway's `core::origin::COMPUTER`). */
const COMPUTER = 'computer';

/**
 * Any ear's question of this home, not one station's (the owner, 27.11): the station, the computer's own ear, and
 * whatever ear comes next — each writes its trip into the journal the same way. The one going, else the last ended.
 * Another station's questions belong to its own scene; a computer's ear (`computer`) is never another station. The half
 * a hub thought for a Турбо's ear (`thoughtIn`) is folded into that ear's trip. `requests` come newest first, as the
 * live feed keeps them.
 */
export function tripOfHome(
    requests: RequestView[],
    { onComputer, station, stations }: { onComputer: boolean; station?: string; stations: string[] },
): { trip?: RequestView; ofHome: RequestView[] } {
    const stationTags = new Set(stations);
    const folded = new Set(requests.flatMap((request) => (request.thoughtIn ? [request.thoughtIn] : [])));
    const ofHome = requests.filter(
        (request) =>
            !folded.has(request.id) &&
            (onComputer || !station || request.device === station || request.device === COMPUTER || !stationTags.has(request.device)),
    );
    const trip = ofHome.find((request) => request.status === 'active') ?? ofHome.find((request) => request.status !== 'active');
    return { trip, ofHome };
}
