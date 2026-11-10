// Мастерская помощника → колонка (docs/plans/desk.md, этап 4; макет WindowWorkshop): плагин пишется и пробуется на
// компьютере, ставится и работает на колонке — и без компьютера. Черновик уходит в мастерскую колонки через проход
// помощника (файлы — как есть, лишние там убираются), а проба и установка идут уже там, рядом с прежней версией.

import { DraftView, InstallPlan, InstallReport, PackageView, api, stationCall } from '../api';

const draftPath = (id: string, path = '') => `/api/v1/workshop/drafts/${encodeURIComponent(id)}${path}`;
const filePath = (id: string, file: string) => draftPath(id, `/files/${file.split('/').map(encodeURIComponent).join('/')}`);

/** Черновик компьютера — в мастерскую колонки: создать там, если нет, переписать файлы, убрать лишние. */
export async function pushDraft(station: string, id: string): Promise<DraftView> {
    const draft = await api.workshopDraft(id);
    let there: DraftView | null = null;
    try {
        there = await stationCall<DraftView>(station, 'GET', draftPath(id));
    } catch {
        there = await stationCall<DraftView>(station, 'POST', '/api/v1/workshop/drafts', { id, kind: draft.kind ?? 'plugin', title: draft.title ?? undefined });
    }
    for (const file of draft.files) {
        const { content } = await api.readDraftFile(id, file.path);
        there = await stationCall<DraftView>(station, 'PUT', filePath(id, file.path), { content });
    }
    const ours = new Set(draft.files.map((file) => file.path));
    for (const file of there?.files ?? []) {
        if (!ours.has(file.path)) there = await stationCall<DraftView>(station, 'DELETE', filePath(id, file.path));
    }
    return there ?? draft;
}

/** Что сделает установка на колонке: версии, права и хосты — показать администратору до «да». */
export function stationPlan(station: string, id: string): Promise<InstallPlan> {
    return stationCall<InstallPlan>(station, 'GET', draftPath(id, '/install'));
}

export function stationInstall(station: string, id: string): Promise<InstallReport> {
    return stationCall<InstallReport>(station, 'POST', draftPath(id, '/install'), { approve: true });
}

export function stationTrial(station: string, id: string, minutes: number): Promise<InstallReport> {
    return stationCall<InstallReport>(station, 'POST', draftPath(id, '/trial'), { approve: true, minutes });
}

/** Какая версия плагина стоит на колонке; null — не стоит или колонки нет на связи. */
export async function stationVersion(station: string, id: string): Promise<string | null> {
    try {
        const view = await stationCall<PackageView>(station, 'GET', `/api/v1/packages/${encodeURIComponent(id)}`);
        return view?.version ?? null;
    } catch {
        return null;
    }
}
