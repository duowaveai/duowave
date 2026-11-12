// Реестр разделов: «Колонка», «Мозг», «Голос», «Дом», «Журнал» — встроенные записи (builtin.tsx), рядом встанут
// разделы плагинов. Оболочка строит навигацию отсюда и не знает, какие разделы есть. Образец — stations/index.ts.

import type { FunctionComponent } from 'preact';

import { can } from '../api';
import { registeredPlugins, useRegistry } from './registry';

export type Icon = FunctionComponent<{ size?: number }>;

/** Раздел пульта: оболочка строит из разделов навигацию, экран раздела рисует его содержимое. */
export interface SectionDef {
    id: string;
    title: string;
    icon: Icon;
    /** Экран раздела. Данные берёт из контекста пульта (usePanel) — пропсов у него нет. */
    Screen: FunctionComponent;
    /** Право из /auth/me, без которого раздела не видно. Нет — раздел виден всем вошедшим. */
    permission?: string;
    /** Порядок в навигации: меньше — раньше. Встроенные — 10, 20, 30…; плагину лучше брать промежутки. */
    order: number;
    /**
     * Где раздел в навигации: 'main' — на панели вкладок и в боковой панели; 'more' — только в боковой панели и по
     * ссылкам (журнал: на панели вкладок для него нет места); 'link' — только по ссылке, без ячейки в доке и в боковой
     * панели («Примерка» черновика: её открывает плитка, полоса или колонка). По умолчанию 'main'.
     */
    nav?: 'main' | 'more' | 'link';
    /** Метка у раздела в навигации (точка новой версии у «Дома»). Компонент — может держать свои хуки. */
    Badge?: FunctionComponent;
}

/** Разделы, которые видит этот вход, по порядку. Раздел с тем же id из плагина позже заменяет прежний. */
export function visibleSections(): SectionDef[] {
    const byId = new Map<string, SectionDef>();
    for (const plugin of registeredPlugins()) for (const section of plugin.sections ?? []) byId.set(section.id, section);
    return [...byId.values()].filter((section) => !section.permission || can(section.permission)).sort((a, b) => a.order - b.order);
}

/** То же для компонентов: перерисовка, когда плагин принёс или убрал раздел. */
export function useSections(): SectionDef[] {
    useRegistry();
    return visibleSections();
}
