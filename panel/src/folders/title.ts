// Подписи папок домашней папки (docs/plans/shared-folders.md): на диске у папок, что делает шлюз, английские имена
// (Photos, Connected) на любом языке; человек читает слова своего интерфейса.

import { tr } from '../i18n';

/** Подпись папки по адресу: у папки, что сделал шлюз (`files/home/Photos`), — слова интерфейса («Фото»), у своей папки
 *  человека и у подключённой — её имя. */
export function folderTitle(address: string): string {
    const parts = address.split('/');
    const name = parts[parts.length - 1] ?? address;
    const names: Record<string, string> = tr.folders.standardNames;
    return parts.length === 3 && parts[0] === 'files' && parts[1] === 'home' ? (names[name] ?? name) : name;
}
