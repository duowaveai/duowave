// Тема черновика, поставленная по слову человека, — сразу на пульте (владелец, 19.11: примерил «NOIR / CUT», нажал
// «Включить» — и пульт вернулся к прежней теме, хотя в «Оформлении» она стояла). Примерка красит пульт временной темой
// `fitting-<id>` (screens/fitting.tsx), мастерская — `preview-<id>`; окно закрылось — временной темы нет, а выбор
// оставался на ней, и пульт брал тему по умолчанию. Теперь выбор переходит на настоящую тему пакета: когда тему ставят
// на примерке, когда отвечают «Применить» в «Работах» и когда её ставят голосом, пока она примерена здесь.

import { api } from '../api';
import { syncThemes } from '../plugins/loader';
import { onWorkshopChange } from '../voice/drafts';
import { currentTheme, setTheme } from './theme';
import { Theme, normalizeTheme } from './themes';

/** Временная тема примерки этой темы: в «Примерке» и в мастерской. */
export function isTrialOf(current: string, theme: string): boolean {
    return current === `fitting-${theme}` || current === `preview-${theme}`;
}

/** Тема из манифеста черновика: первая из `contributes.themes`; нет такой — null. */
export async function draftTheme(draft: string): Promise<Theme | null> {
    const manifest = await api.readDraftFile(draft, 'manifest.json');
    return normalizeTheme(JSON.parse(manifest.content)?.contributes?.themes?.[0]);
}

/** Поставленную тему черновика — на пульт: темы пакетов перечитаны, выбор — её id. false — темы в черновике нет. */
export async function takeDraftTheme(draft: string): Promise<boolean> {
    const found = await draftTheme(draft).catch(() => null);
    if (!found) return false;
    // Сначала пакет, потом выбор: иначе между ними пульт мигнёт темой по умолчанию.
    await syncThemes();
    setTheme(found.id);
    return true;
}

/** Черновики-темы, которые человек велел поставить не на примерке («Применить» в «Работах»): ждут `installed`. */
const wanted = new Set<string>();

/** Взять тему черновика, когда шлюз скажет, что она поставлена. */
export function takeThemeWhenInstalled(draft: string): void {
    wanted.add(draft);
}

// Черновик поставлен: тема, которую велели применить, — на пульт; поставили голосом, пока она примерена здесь, — тоже.
onWorkshopChange((draft, change) => {
    if (change !== 'installed' && change !== 'updated') return;
    if (wanted.delete(draft)) {
        void takeDraftTheme(draft);
        return;
    }
    const current = currentTheme().id;
    if (!current.startsWith('fitting-') && !current.startsWith('preview-')) return;
    void draftTheme(draft)
        .then((found) => found && isTrialOf(currentTheme().id, found.id) && takeDraftTheme(draft))
        .catch(() => undefined);
});
