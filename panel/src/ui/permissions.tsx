// «Разрешения» (docs/plans/permissions.md): что система этого компьютера должна разрешить приложению DuoWave. Строки
// с кнопкой у каждой, галочки ставятся сами (список читается раз в секунду, пока строки на экране, и при возврате в
// окно). Нарисованное окно macOS показывает заранее, что сейчас появится и где нажать (как у AI Dictation).
// Используется в шаге первых шагов «Разрешения», в шаге «Где» (если что-то не пустило поиск), в «Настройках» →
// «Доступ» и на странице окна-подсказки рядом с «Системными настройками».

import { useEffect, useRef, useState } from 'preact/hooks';

import { type PermissionItem, type PermissionRow, type PermissionsView, api, problemOf } from '../api';
import { tr } from '../i18n';
import './permissions.css';

/** Строке ещё нужно разрешение: система отказала или ещё не спрашивала. `unknown` не мешает никому. */
export const needs = (row: PermissionRow): boolean => row.state === 'denied' || row.state === 'notAsked';

/** Сколько после «Разрешить» строка ждёт ответа в окне macOS, прежде чем предложить настройки. */
const WAIT_MS = 20_000;

export interface Permissions {
    view: PermissionsView | null;
    /** Список не прочитался (старый шлюз, нет сети). */
    failed: boolean;
    /** Когда нажали «Разрешить» у строки: пока ждём ответа в окне macOS. */
    asked: Partial<Record<PermissionItem, number>>;
    busy: PermissionItem | null;
    problem: string | null;
    act: (row: PermissionRow, how?: 'ask' | 'open') => Promise<void>;
    reread: () => void;
}

/** Разрешения этого компьютера; `active` — читать раз в секунду. */
export function usePermissions(active = true): Permissions {
    const [view, setView] = useState<PermissionsView | null>(null);
    const [failed, setFailed] = useState(false);
    const [asked, setAsked] = useState<Partial<Record<PermissionItem, number>>>({});
    const [busy, setBusy] = useState<PermissionItem | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [round, setRound] = useState(0);
    useEffect(() => {
        if (!active) return;
        let alive = true;
        const read = () =>
            api
                .permissions()
                .then((next) => {
                    if (!alive) return;
                    setView(next);
                    setFailed(false);
                })
                .catch(() => alive && setFailed(true));
        void read();
        const timer = window.setInterval(() => void read(), 1000);
        const onFocus = () => void read();
        window.addEventListener('focus', onFocus);
        return () => {
            alive = false;
            window.clearInterval(timer);
            window.removeEventListener('focus', onFocus);
        };
    }, [active, round]);
    const act = async (row: PermissionRow, how?: 'ask' | 'open') => {
        const action = how ?? row.action;
        if (!action) return;
        setBusy(row.item);
        setProblem(null);
        try {
            if (action === 'ask') {
                setAsked((now) => ({ ...now, [row.item]: Date.now() }));
                setView(await api.askPermission(row.item));
            } else if (action === 'open') {
                setView(await api.openPermission(row.item, view?.os === 'mac'));
            } else {
                await api.allowWindowsFirewall();
                setRound((now) => now + 1);
            }
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(null);
        }
    };
    return { view, failed, asked, busy, problem, act, reread: () => setRound((now) => now + 1) };
}

/** Строки, которые можно показать с кнопками: приложение есть и пульт открыт на этом компьютере. */
export const actionable = (view: PermissionsView | null): boolean => !!view?.app && !!view.here;

// ─── Значки ────────────────────────────────────────────────────

const ICONS: Record<PermissionItem, preact.ComponentChildren> = {
    localNetwork: (
        <>
            <circle cx="12" cy="12" r="2" />
            <path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M4.9 4.9a10 10 0 0 0 0 14.2M19.1 4.9a10 10 0 0 1 0 14.2" />
        </>
    ),
    microphone: (
        <>
            <rect x="9" y="3" width="6" height="11" rx="3" />
            <path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21" />
        </>
    ),
    firewall: <path d="M12 3l7.5 3v5.5c0 4.5-3.2 8.2-7.5 9.5-4.3-1.3-7.5-5-7.5-9.5V6z" />,
    networkProfile: (
        <>
            <circle cx="12" cy="12" r="9" />
            <path d="M3 12h18M12 3c2.6 2.7 2.6 15.3 0 18M12 3c-2.6 2.7-2.6 15.3 0 18" />
        </>
    ),
};

function Icon({ item }: { item: PermissionItem }) {
    return (
        <svg class="perm-ico" viewBox="0 0 24 24" aria-hidden="true">
            {ICONS[item]}
        </svg>
    );
}

// ─── Строки ────────────────────────────────────────────────────

function title(row: PermissionRow): string {
    const t = tr.permissions;
    switch (row.item) {
        case 'localNetwork':
            return t.localNetwork;
        case 'microphone':
            return t.microphone;
        case 'firewall':
            return t.firewall;
        case 'networkProfile':
            return t.networkProfile;
    }
}

function why(row: PermissionRow, os: PermissionsView['os']): string {
    const t = tr.permissions;
    switch (row.item) {
        case 'localNetwork':
            return t.localNetworkWhy;
        case 'microphone':
            return os === 'windows' && row.state === 'denied' ? t.microphoneWhyWindows : t.microphoneWhy;
        case 'firewall':
            return t.firewallWhy;
        case 'networkProfile':
            return t.networkProfileWhy((row.networks ?? []).join('», «'));
    }
}

/** Что сделать в системе, когда кнопка открыла настройки: Windows — по шагам, Mac — если окна не было. */
function steps(row: PermissionRow, os: PermissionsView['os'], waiting: boolean): string | null {
    const t = tr.permissions;
    if (row.state === 'granted') return null;
    if (os === 'windows') {
        if (row.item === 'microphone') return t.stepsMicrophoneWindows;
        if (row.item === 'networkProfile') return t.stepsNetworkWindows;
        if (row.item === 'firewall') return t.stepsFirewall;
        return null;
    }
    if (os === 'mac' && (row.state === 'denied' || waiting)) return t.noWindowMac;
    return null;
}

function label(action: PermissionRow['action']): string {
    const t = tr.permissions;
    return action === 'ask' ? t.ask : action === 'allowFirewall' ? t.allowFirewall : t.open;
}

/** Кнопки по месту: в первых шагах — их капсулы, в пульте — его кнопки. */
const BUTTONS = {
    onb: { main: 'dw-btn dw-btn-accent', light: 'dw-btn dw-btn-light', quiet: 'btn-ghost' },
    panel: { main: 'btn btn-main btn-sm', light: 'btn btn-plain btn-sm', quiet: 'btn btn-plain btn-sm' },
};

/**
 * Строки разрешений. `items` — какие показать и в каком порядке (по умолчанию все); `order` — по очереди: кнопка
 * следующей строки появляется, когда у предыдущей есть ответ (шаг «Разрешения» на Mac). `look` — где стоят строки.
 */
export function PermissionRows({
    perms,
    items,
    order = false,
    look = 'panel',
}: {
    perms: Permissions;
    items?: PermissionItem[];
    order?: boolean;
    look?: 'onb' | 'panel';
}) {
    const t = tr.permissions;
    const buttons = BUTTONS[look];
    const view = perms.view;
    if (!view) return null;
    const rows = items
        ? items.map((item) => view.items.find((row) => row.item === item)).filter((row): row is PermissionRow => !!row)
        : view.items;
    const can = actionable(view);
    let blocked = false;
    return (
        <div class="perm-rows">
            {rows.map((row) => {
                const askedAt = perms.asked[row.item];
                const waiting = row.state !== 'granted' && !!askedAt && Date.now() - askedAt < WAIT_MS;
                const turn = !blocked;
                if (order && needs(row) && !waiting) blocked = true;
                const hint = can ? steps(row, view.os, waiting) : null;
                return (
                    <div key={row.item} class={`perm-row${row.state === 'granted' ? ' is-ok' : ''}`}>
                        <span class="perm-icon">
                            <Icon item={row.item} />
                        </span>
                        <span class="perm-text">
                            <b>{title(row)}</b>
                            <span class="perm-why">{why(row, view.os)}</span>
                            {hint && <span class="perm-hint">{hint}</span>}
                            {/* Ждём ответа, а окна, может быть, не было: настройки — здесь же, тише кнопки. */}
                            {can && waiting && view.os === 'mac' && (
                                <button type="button" class={`${buttons.quiet} perm-link`} onClick={() => void perms.act(row, 'open')}>
                                    {t.open}
                                </button>
                            )}
                        </span>
                        <span class="perm-side">
                            {row.state === 'granted' ? (
                                <span class="perm-ok">
                                    <svg viewBox="0 0 24 24" aria-hidden="true">
                                        <path d="M5 12.5l4.5 4.5L19 7.5" />
                                    </svg>
                                    {t.granted}
                                </span>
                            ) : waiting ? (
                                <span class="perm-wait" role="status">
                                    <span class="perm-spin" aria-hidden="true" />
                                    {t.waiting}
                                </span>
                            ) : can && row.action && turn ? (
                                <button
                                    type="button"
                                    class={`${row.action === 'ask' ? buttons.main : buttons.light} perm-btn`}
                                    disabled={perms.busy !== null}
                                    aria-busy={perms.busy === row.item || undefined}
                                    onClick={() => void perms.act(row)}
                                >
                                    {label(row.action)}
                                </button>
                            ) : null}
                        </span>
                    </div>
                );
            })}
            {!can && view.app && <p class="small perm-note">{t.notHere}</p>}
            {perms.problem && (
                <p class="small perm-note" role="alert">
                    {perms.problem}
                </p>
            )}
        </div>
    );
}

/**
 * Следит за строками и зовёт `onGranted`, когда одна из нужных стала разрешённой (шаг «Где»: поиск колонки
 * повторяется сам; шаг «Разрешения»: всё разрешено — дальше).
 */
export function useGranted(perms: Permissions, items: PermissionItem[], onGranted: (item: PermissionItem) => void): void {
    const before = useRef<Partial<Record<PermissionItem, boolean>>>({});
    useEffect(() => {
        for (const row of perms.view?.items ?? []) {
            if (!items.includes(row.item)) continue;
            const was = before.current[row.item];
            const now = row.state === 'granted';
            if (was === false && now) onGranted(row.item);
            before.current[row.item] = now;
        }
    }, [perms.view]);
}

// ─── Нарисованное окно macOS ───────────────────────────────────

/** Ладонь в углу значка: так macOS помечает вопрос о конфиденциальности. */
function HandBadge() {
    return (
        <span class="perm-alert-badge">
            <svg viewBox="0 0 16 16" aria-hidden="true">
                <rect x="4.1" y="3.2" width="1.9" height="7" rx=".95" />
                <rect x="6.3" y="1.8" width="1.9" height="8" rx=".95" />
                <rect x="8.5" y="2.4" width="1.9" height="7.6" rx=".95" />
                <rect x="10.7" y="4" width="1.8" height="6.4" rx=".9" />
                <rect x="4.1" y="7.4" width="8.4" height="6.6" rx="3.2" />
                <rect x="1.9" y="8.4" width="1.8" height="4.6" rx=".9" transform="rotate(-32 2.8 10.7)" />
            </svg>
        </span>
    );
}

/** Значок программы в окне macOS: локальная сеть — синий с глобусом, микрофон — серый с микрофоном. */
function PromptIcon({ item }: { item: 'localNetwork' | 'microphone' }) {
    return (
        <span class={`perm-alert-icon is-${item}`}>
            <svg viewBox="0 0 48 48" aria-hidden="true">
                {item === 'localNetwork' ? (
                    <>
                        <circle cx="24" cy="24" r="14" />
                        <ellipse cx="24" cy="24" rx="6.5" ry="14" />
                        <path d="M10 24h28M12.4 16.5h23.2M12.4 31.5h23.2M24 10v28" />
                    </>
                ) : (
                    <>
                        <rect x="18.5" y="8" width="11" height="21" rx="5.5" />
                        <path d="M13.5 22.5a10.5 10.5 0 0 0 21 0M24 33v7M18 40h12" />
                    </>
                )}
            </svg>
            <HandBadge />
        </span>
    );
}

/**
 * Окно macOS, которое сейчас появится, с курсором на «Разрешить» (как у AI Dictation). Нарисовано по настоящим окнам
 * macOS 26 (снимки владельца 27.11): светлое и тёмное, как система; значок, ладонь, кнопки-капсулы, рамка фокуса на
 * «Не разрешать», у микрофона — «?». Рисунок, не снимок: язык пульта.
 */
export function SystemPrompt({ item }: { item: 'localNetwork' | 'microphone' }) {
    const t = tr.permissions;
    return (
        <figure class="perm-picture">
            <div class="perm-alert" aria-hidden="true">
                {item === 'microphone' && <span class="perm-alert-help">?</span>}
                <PromptIcon item={item} />
                <b>{item === 'localNetwork' ? t.promptLocalNetwork : t.promptMicrophone}</b>
                <span class="perm-alert-body">{item === 'localNetwork' ? t.promptLocalNetworkBody : t.promptMicrophoneBody}</span>
                <span class="perm-alert-buttons">
                    <i class="is-focus">{t.promptDeny}</i>
                    <i class="is-allow">
                        {t.promptAllow}
                        <svg class="perm-cursor" viewBox="0 0 24 24">
                            <path d="M5 3l14 8.5-6.2 1.3 3.7 6.9-2.6 1.3-3.7-6.9L5 18.5z" />
                        </svg>
                    </i>
                </span>
            </div>
            <figcaption class="small">{t.pictureCaption}</figcaption>
        </figure>
    );
}
