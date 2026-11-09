// Оболочки по имени из темы (Theme.layout). Незнакомое имя — панель вкладок: тема из плагина не может оставить пульт
// без навигации.

import type { FunctionComponent } from 'preact';

import type { ShellProps } from './shell';
import { SidebarShell } from './sidebar-shell';
import { TabbarShell } from './tabbar-shell';

const SHELLS: Record<string, FunctionComponent<ShellProps>> = {
    tabbar: TabbarShell,
    sidebar: SidebarShell,
};

/** Оболочка темы и её имя. */
export function shellFor(layout: string): { id: string; Shell: FunctionComponent<ShellProps> } {
    return layout in SHELLS ? { id: layout, Shell: SHELLS[layout] } : { id: 'tabbar', Shell: TabbarShell };
}
