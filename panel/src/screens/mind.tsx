// Куда ведут ссылки дашборда и экранов: вкладки прежних «Мозга» и «Голоса». Их места теперь в «Персонаже», «Навыках»
// и «Настройках» (screens/places.tsx, settingsPlace); имена вкладок остались — ими говорят дашборд и подключения.

import type { ModelsTab } from './models';

export type MindTab = 'routes' | 'brain' | 'characters';

/** Куда ведут ссылки дашборда: вкладка «Мозга» или «Голоса». */
export type SettingsTab = MindTab | ModelsTab | 'character';

export const isMindTab = (tab: SettingsTab): tab is MindTab => tab === 'routes' || tab === 'brain' || tab === 'characters';
