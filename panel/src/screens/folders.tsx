// «Папки» (docs/plans/shared-folders.md): домашняя папка DuoWave — `~/DuoWave` в приложении, обычная папка в Finder.
// Голос, агент и приложения видят только её: Фото, Документы, Видео, Загрузки, Картинки; музыку и сказки дома; проекты;
// и «Подключено» — флешку или папку с диска, подключённые по желанию человека (ссылкой: всё остаётся на месте). Здесь
// видно, что в каждой папке, кто её видит и что из неё уходит из дома; администратор меняет галочки, подключает и
// отключает. Голосом по папкам ходит приложение «Папки».

import { useEffect, useRef, useState } from 'preact/hooks';

import { FolderItem, FolderPage, FolderRole, FolderRule, FolderView, FoldersView, api, can, fetchFolderPreview, problemOf } from '../api';
import { formatList, formatNumber, tr } from '../i18n';
import { foldersChanged, useFoldersVersion } from '../folders/state';
import { IconChevronLeft, IconChevronRight, IconFolder } from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { Action } from '../ui/cards';
import { ask } from '../ui/confirm';
import { Switch } from '../ui/switch';
import { FolderOffers } from './folder-offers';
import './folders.css';

const groups = (): Array<{ kind: FolderView['kind']; title: string; empty?: string }> => [
    { kind: 'home', title: tr.folders.groupHome },
    { kind: 'content', title: tr.folders.groupContent },
    { kind: 'attached', title: tr.folders.groupAttached, empty: tr.folders.groupAttachedEmpty },
    { kind: 'project', title: tr.folders.groupProject },
];

const roleTitles = (): Record<FolderRole, string> => ({ admin: tr.folders.roleAdmin, member: tr.folders.roleMember, guest: tr.folders.roleGuest });

/** The section's tab while no folder is open (a folder's tab is its address, files/…). */
const LIST = 'list';

/** Размер словами: «18 ГБ», «340 КБ». */
function sizeWords(bytes: number): string {
    const units = tr.folders.sizeUnits;
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024;
        unit += 1;
    }
    return `${value >= 10 || unit === 0 ? Math.round(value) : formatNumber(value, { maximumFractionDigits: 1 })} ${units[unit]}`;
}

/** Кто видит папку словами: «вы и семья». */
function rolesWords(roles: FolderRole[]): string {
    const titles = roleTitles();
    return formatList((['admin', 'member', 'guest'] as FolderRole[]).filter((role) => role === 'admin' || roles.includes(role)).map((role) => titles[role]));
}

export function FoldersSection() {
    const version = useFoldersVersion();
    const [view, setView] = useState<FoldersView | null>(null);
    const [error, setError] = useState<string | null>(null);
    // The list is a tab of its own: an empty tab is never remembered (app.tsx settle), so «назад» to '' kept the card open.
    const [opened, setOpened] = useSectionTab<string>('folders', LIST);
    useEffect(() => {
        let alive = true;
        api.folders()
            .then((next) => {
                if (!alive) return;
                setView(next);
                setError(null);
            })
            .catch((e) => alive && setError(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [version]);
    if (error && !view)
        return (
            <section class="card glass" role="alert">
                <p class="t-meta m-0">{tr.folders.loadError(error)}</p>
            </section>
        );
    if (!view) return null;
    const folder = view.folders.find((entry) => entry.address === opened);
    if (folder) return <FolderCard view={view} folder={folder} onBack={() => setOpened(LIST)} />;
    return <FoldersList view={view} onOpen={setOpened} />;
}

function FoldersList({ view, onOpen }: { view: FoldersView; onOpen: (address: string) => void }) {
    const admin = can('plugins.manage');
    return (
        <div class="folders-page">
            <section class="card glass card-list" aria-labelledby="folders-h">
                <div class="section-head">
                    <h2 id="folders-h" class="t-title-xs m-0">
                        {tr.folders.homeTitle}
                    </h2>
                </div>
                <p class="t-caption m-0 mb-8">
                    {view.visible
                        ? tr.folders.homeVisible
                        : tr.folders.homeHidden}
                    {view.home ? ` ${view.home}` : ''}
                </p>
                <div class="flex flex-wrap gap-8">
                    {view.local && view.folders[0] && <Action label={tr.folders.revealInFinder} onClick={() => void api.folderReveal(view.folders[0].address).catch(() => {})} />}
                </div>
            </section>
            <FolderOffers offers={view.offers} />
            {admin && view.attach && <AttachCard view={view} />}
            {admin && view.notes && view.notes.length > 0 && (
                <section class="card glass" role="status">
                    {view.notes.map((note) => (
                        <p key={note} class="t-caption m-0">
                            {note}
                        </p>
                    ))}
                </section>
            )}
            {groups().map((group) => {
                const folders = view.folders.filter((entry) => entry.kind === group.kind);
                if (folders.length === 0 && !group.empty) return null;
                return (
                    <section key={group.kind} class="card glass card-list" aria-label={group.title}>
                        <div class="section-head">
                            <h2 class="t-title-xs m-0">{group.title}</h2>
                        </div>
                        {folders.length === 0 && <p class="t-caption m-0">{group.empty}</p>}
                        <div class="w-rows">
                            {folders.map((folder) => (
                                <button key={folder.address} class="w-row w-row-btn" onClick={() => onOpen(folder.address)}>
                                    <span class="folders-icon" aria-hidden="true">
                                        <IconFolder size={18} />
                                    </span>
                                    <span class="flex-1 min-w-0 flex flex-col">
                                        <span class="w-text">{folder.name}</span>
                                        <span class="w-cap">
                                            {folder.here ? tr.folders.seenBy(rolesWords(folder.rule.roles)) : tr.folders.notHere}
                                            {folder.apps && folder.apps.length > 0
                                                ? ` · ${tr.folders.appsCount(folder.apps.length)}`
                                                : ''}
                                        </span>
                                    </span>
                                    <IconChevronRight size={16} />
                                </button>
                            ))}
                        </div>
                    </section>
                );
            })}
        </div>
    );
}

/** «Подключить папку…»: окно выбора в приложении DuoWave; в браузере на этом компьютере — путь словами. */
function AttachCard({ view }: { view: FoldersView }) {
    const [path, setPath] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const run = async (action: () => Promise<unknown>): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            await action();
            setPath('');
            foldersChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <section class="card glass" aria-labelledby="attach-h">
            <h2 id="attach-h" class="t-title-xs m-0 mb-8">
                {tr.folders.attachTitle}
            </h2>
            <p class="t-caption m-0 mb-8">
                {tr.folders.attachNote}
            </p>
            {!view.local ? (
                <p class="t-caption m-0">{tr.folders.attachOnlyOnComputer}</p>
            ) : view.pick ? (
                <Action label={tr.folders.pickFolder} busy={busy} onClick={() => void run(() => api.folderPick())} />
            ) : (
                <form
                    class="flex flex-wrap gap-8"
                    onSubmit={(e) => {
                        e.preventDefault();
                        if (path.trim()) void run(() => api.folderAttach(path.trim()));
                    }}
                >
                    <input class="field flex-1" placeholder={tr.folders.pathPlaceholder} value={path} onInput={(e) => setPath((e.target as HTMLInputElement).value)} aria-label={tr.folders.pathLabel} />
                    <Action label={tr.folders.attach} busy={busy} disabled={!path.trim()} onClick={() => path.trim() && void run(() => api.folderAttach(path.trim()))} />
                </form>
            )}
            {error && (
                <p class="t-caption c-dot-offline m-0 mt-8" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}

function FolderCard({ view, folder, onBack }: { view: FoldersView; folder: FolderView; onBack: () => void }) {
    const admin = can('plugins.manage');
    const [size, setSize] = useState<{ files: number; bytes: number; partial: boolean } | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // A folder opens at its top, not where the list was scrolled to.
    const top = useRef<HTMLButtonElement>(null);
    useEffect(() => top.current?.scrollIntoView({ block: 'start' }), [folder.address]);
    useEffect(() => {
        if (!admin || !folder.here) return;
        let alive = true;
        api.folderSize(folder.address)
            .then((counted) => alive && setSize(counted))
            .catch(() => {});
        return () => {
            alive = false;
        };
    }, [folder.address, folder.here]);
    const rule = folder.rule;
    const change = async (patch: Parameters<typeof api.folderRule>[1]): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            await api.folderRule(folder.address, patch);
            foldersChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const toggleRole = (role: FolderRole, on: boolean) => {
        const roles = new Set<FolderRole>(rule.roles);
        if (on) roles.add(role);
        else roles.delete(role);
        void change({ roles: [...roles] });
    };
    const detach = async () => {
        if (!(await ask({ title: tr.folders.detachTitle(folder.name), text: tr.folders.detachText, ok: tr.folders.detach }))) return;
        setBusy(true);
        try {
            await api.folderDetach(folder.name);
            foldersChanged();
            onBack();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <>
            <button ref={top} class="deeper-back" onClick={onBack}>
                <IconChevronLeft size={16} />
                {tr.folders.back}
            </button>
            <section class="card glass" aria-labelledby="folder-h">
                <h2 id="folder-h" class="t-title-xs m-0">
                    {folder.name}
                </h2>
                <p class="t-caption m-0 mb-8">
                    {folder.here ? (folder.path ?? folder.address) : tr.folders.folderGone}
                    {size ? tr.folders.sizeLine(size.files, sizeWords(size.bytes), size.partial) : ''}
                </p>
                <div class="flex flex-wrap gap-8">
                    {view.local && folder.here && <Action label={tr.folders.revealInFinder} onClick={() => void api.folderReveal(folder.address).catch((e) => setError(problemOf(e)))} />}
                    {admin && folder.kind === 'attached' && <Action label={tr.folders.detach} disabled={busy} onClick={() => void detach()} />}
                </div>
                {error && (
                    <p class="t-caption c-dot-offline m-0 mt-8" role="alert">
                        {error}
                    </p>
                )}
            </section>
            {admin && (
                <section class="card glass" aria-label={tr.folders.visibilityLabel}>
                    <h3 class="t-title-xs m-0 mb-8">{tr.folders.whoSees}</h3>
                    <Tick label={tr.folders.tickFamily} on={rule.roles.includes('member')} disabled={busy} onChange={(on) => toggleRole('member', on)} />
                    <Tick label={tr.folders.tickGuests} on={rule.roles.includes('guest')} disabled={busy} onChange={(on) => toggleRole('guest', on)} />
                    {folder.kind === 'attached' && (
                        <>
                            <h3 class="t-title-xs m-0 mt-8 mb-8">{tr.folders.attachedTitle}</h3>
                            <Tick label={tr.folders.tickWrite} on={rule.write} disabled={busy} onChange={(on) => void change({ write: on })} />
                            {rule.how === 'drive' && <Tick label={tr.folders.tickRemember} on={rule.remember} disabled={busy} onChange={(on) => void change({ remember: on })} />}
                        </>
                    )}
                    <h3 class="t-title-xs m-0 mt-8 mb-8">{tr.folders.leavesHome}</h3>
                    <Tick
                        label={tr.folders.tickModel}
                        on={rule.model}
                        disabled={busy}
                        amber={view.cloudModel ? tr.folders.modelCloud : undefined}
                        hint={view.cloudModel ? undefined : tr.folders.modelLocal}
                        onChange={(on) => void change({ model: on })}
                    />
                    <Tick
                        label={tr.folders.tickSpeak}
                        on={rule.speak}
                        disabled={busy}
                        amber={view.yandexVoice ? tr.folders.speakYandex : undefined}
                        hint={view.yandexVoice ? undefined : tr.folders.speakLocal}
                        onChange={(on) => void change({ speak: on })}
                    />
                    <Tick label={tr.folders.tickRemote} on={rule.remote} disabled={busy} hint={tr.folders.remoteHint} onChange={(on) => void change({ remote: on })} />
                    {folder.apps && folder.apps.length > 0 && (
                        <p class="t-caption m-0 mt-8">
                            {tr.folders.appsUse(
                                folder.apps
                                    .map((reach) => tr.folders.appReach(reach.title, reach.level === 'edit' ? tr.folders.levelEdit : reach.level === 'call' ? tr.folders.levelCall : tr.folders.levelList))
                                    .join(', '),
                            )}
                        </p>
                    )}
                </section>
            )}
            {folder.here && <Browser root={folder.address} />}
        </>
    );
}

function Tick({ label, on, disabled, amber, hint, onChange }: { label: string; on: boolean; disabled?: boolean; amber?: string; hint?: string; onChange: (on: boolean) => void }) {
    return (
        <div class="flex flex-col gap-4 mb-8">
            <div class="flex items-center gap-12">
                <span class="t-body-sm flex-1">{label}</span>
                <Switch on={on} label={label} disabled={disabled} onChange={onChange} />
            </div>
            {amber && <span class="t-caption folders-amber">{amber}</span>}
            {hint && <span class="t-caption">{hint}</span>}
        </div>
    );
}

/** Что в папке: страницами, папки первыми; картинки — уменьшенными. */
function Browser({ root }: { root: string }) {
    const version = useFoldersVersion();
    const [at, setAt] = useState(root);
    const [page, setPage] = useState<FolderPage | null>(null);
    const [items, setItems] = useState<FolderItem[]>([]);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => setAt(root), [root]);
    useEffect(() => {
        let alive = true;
        api.folderBrowse(at)
            .then((first) => {
                if (!alive) return;
                setPage(first);
                setItems(first.items);
                setError(null);
            })
            .catch((e) => alive && setError(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [at, version]);
    const more = () => {
        if (page?.next == null) return;
        api.folderBrowse(at, page.next)
            .then((next) => {
                setPage(next);
                setItems((shown) => [...shown, ...next.items]);
            })
            .catch((e) => setError(problemOf(e)));
    };
    const trail = at.slice(root.length).split('/').filter(Boolean);
    return (
        <section class="card glass" aria-label={tr.folders.browserLabel}>
            <div class="flex flex-wrap items-center gap-4 mb-8 t-caption">
                <button class="link-button" onClick={() => setAt(root)}>
                    {root.split('/').pop()}
                </button>
                {trail.map((part, index) => (
                    <span key={`${index}-${part}`}>
                        {' › '}
                        <button class="link-button" onClick={() => setAt([root, ...trail.slice(0, index + 1)].join('/'))}>
                            {part}
                        </button>
                    </span>
                ))}
                {page && <span>{tr.folders.itemsTotal(page.total)}</span>}
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
            {page && page.total === 0 && <p class="t-caption m-0">{tr.folders.empty}</p>}
            <div class="folders-grid">
                {items.map((item) => (
                    <button key={item.address} class="folders-item" onClick={() => item.folder && setAt(item.address)} disabled={!item.folder} title={item.name}>
                        {item.kind === 'image' ? <Thumb address={item.address} /> : <span class={`folders-kind folders-kind-${item.kind}`}>{item.folder ? <IconFolder size={28} /> : item.kind}</span>}
                        <span class="folders-name">{item.name}</span>
                    </button>
                ))}
            </div>
            {page?.next != null && <Action label={tr.folders.more} onClick={more} />}
        </section>
    );
}

function Thumb({ address }: { address: string }) {
    const [url, setUrl] = useState<string | null>(null);
    useEffect(() => {
        let made: string | null = null;
        let alive = true;
        fetchFolderPreview(address, 256)
            .then((blob) => {
                if (!alive) return;
                made = URL.createObjectURL(blob);
                setUrl(made);
            })
            .catch(() => {});
        return () => {
            alive = false;
            if (made) URL.revokeObjectURL(made);
        };
    }, [address]);
    return url ? <img class="folders-thumb" src={url} alt="" loading="lazy" /> : <span class="folders-kind folders-kind-image">{tr.folders.photo}</span>;
}

/** Открыть папку в разделе «Папки» из другого места пульта (плитка флешки на столе). */
export function useOpenFolder(): (address: string) => void {
    const panel = usePanel();
    return (address: string) => panel.go('folders', address);
}

export type { FolderRule };
