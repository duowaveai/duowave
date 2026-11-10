// Оболочки по виду (theme.ts): стол и док или боковая панель. Незнакомое имя — стол: вид из чужой записи не может
// оставить пульт без навигации; 'tabbar' прежних тем — тоже стол (док и есть бывшая панель вкладок).

import type { FunctionComponent } from 'preact';

import { DeskShell } from './desk-shell';
import type { ShellProps } from './shell';
import { SidebarShell } from './sidebar-shell';

/** Оболочка вида и её имя. */
export function shellFor(view: string): { id: 'desk' | 'sidebar'; Shell: FunctionComponent<ShellProps> } {
    return view === 'sidebar' ? { id: 'sidebar', Shell: SidebarShell } : { id: 'desk', Shell: DeskShell };
}
