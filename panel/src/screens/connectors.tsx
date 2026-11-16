import { useEffect, useState } from 'preact/hooks';

import { ConnectorSettings, ConnectorView, ContactView, TelegramStranger, api, can, problemOf } from '../api';
import { type BrandId } from '../brand-icons';
import { IconEar, IconGlobe, IconHome, IconLinks, IconMcp, IconPhone, IconRobot, IconSend, IconSparkle, IconSpeaker, IconVoice } from '../icons';
import { tr } from '../i18n';
import { timeAgo } from '../time';
import { BrandMark } from '../ui/brand';
import { Action } from '../ui/cards';
import { CheckLine, ContactDraft, ContactsField, EnvNote, SecretField, TextField, contactsInput } from '../ui/fields';
import { SaveBar, Sheet, SkeletonRows, WinRow, WinSection } from '../ui/sheet';
import { Switch } from '../ui/switch';
import type { SettingsTab } from './mind';
import { NewsSheet } from './news';
import { ask } from '../ui/confirm';
import { saveLabel } from '../via';
import { onComputer } from '../voice-input';
import { byMode } from '../words';

/** Работает и настроено — лайм, не настроено и выключено — сланец, нет связи — коралл, «скоро» — чего в этой сборке ещё нет. */
const statePill = (): Record<ConnectorView['status']['state'], { cls: string; label: string }> => ({
    ready: { cls: 'pill-own', label: tr.connectors.stateReady },
    configured: { cls: 'pill-own', label: tr.connectors.stateConfigured },
    unset: { cls: 'pill-off', label: tr.connectors.stateUnset },
    off: { cls: 'pill-off', label: tr.connectors.stateOff },
    soon: { cls: 'pill-off', label: tr.connectors.stateSoon },
    error: { cls: 'pill-error', label: tr.connectors.stateError },
});

/** Знак подключения: сервис, если его знак есть (Telegram, Home Assistant…), иначе наш значок по виду. У Яндекса знака нет. */
const CONNECTOR_BRANDS: Record<string, BrandId> = {
    'home-assistant': 'homeassistant',
    telegram: 'telegram',
    livekit: 'livekit',
    sip: 'asterisk',
    news: 'rss',
    'piper-registry': 'huggingface',
};
const KIND_ICONS: Record<string, (props: { size?: number }) => preact.JSX.Element> = {
    station: IconSpeaker,
    llm: IconSparkle,
    stt: IconEar,
    tts: IconVoice,
    lookup: IconGlobe,
    calls: IconPhone,
    messenger: IconSend,
    smart_home: IconHome,
    mcp: IconMcp,
    agent: IconRobot,
};

export function ConnectorIcon({ id, kind, size = 38 }: { id: string; kind: string; size?: number }) {
    const brand = CONNECTOR_BRANDS[id];
    const Glyph = KIND_ICONS[kind] ?? IconLinks;
    return (
        <span class="route-icon" aria-hidden="true" style={{ width: size, height: size, flex: 'none' }}>
            {brand ? <BrandMark id={brand} size={Math.round(size / 2)} /> : <Glyph size={Math.round(size / 2)} />}
        </span>
    );
}

/**
 * Подключения из реестра шлюза: облачные — янтарные (DESIGN.md: то, что уходит из дома). Строка того, что
 * настраивается здесь, открывает свой лист; модель ведёт в «Мозг», голос и распознавание — в «Голос».
 */
export function Connectors({
    onOpenSettings,
    openId,
    onOpened,
}: {
    onOpenSettings: (tab: SettingsTab) => void;
    /** Открыть этот лист сразу (ссылка с дашборда). */
    openId?: string | null;
    onOpened?: () => void;
}) {
    const [list, setList] = useState<ConnectorView[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [open, setOpen] = useState<string | null>(null);
    const mayManage = can('connectors.manage');
    const read = () =>
        api
            .connectors()
            .then((answer) => {
                setList(answer.connectors);
                setError(null);
            })
            .catch((e) => setError(problemOf(e)));
    useEffect(() => {
        void read();
        const id = setInterval(read, 15_000);
        return () => clearInterval(id);
    }, []);
    useEffect(() => {
        if (!openId) return;
        if (mayManage) setOpen(openId);
        onOpened?.();
    }, [openId]);

    const go = (view: ConnectorView): (() => void) | null => {
        switch (view.manifest.setup) {
            case 'panel':
                return mayManage ? () => setOpen(view.manifest.id) : null;
            case 'brain':
                return () => onOpenSettings('brain');
            case 'voice':
                return () => onOpenSettings(view.manifest.kind === 'stt' ? 'recognition' : 'voice');
            default:
                return null;
        }
    };
    const opened = list?.find((view) => view.manifest.id === open);
    // Окна — вне стеклянной карточки: её backdrop-filter стал бы опорой их position: fixed.
    return (
        <>
            <section class="card glass card-list card-list-tight" aria-labelledby="connectors-h">
                <div class="section-head">
                    <h2 id="connectors-h" class="t-title-xs m-0">
                        {tr.connectors.title}
                    </h2>
                    <span class="t-meta">{mayManage ? tr.connectors.tapToSetUp : tr.connectors.mcpBelow}</span>
                </div>
                {error && (
                    <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                        {error}
                    </p>
                )}
                {!list && !error && (
                    <p class="t-meta m-0 mb-8">
                        {tr.connectors.askingGateway}
                    </p>
                )}
                {list
                    // Модель, слух и голос живут у себя — «Персонаж → Мозг», «Настройки → Колонка → Слух», «Персонаж → Голос»
                    // (docs/plans/menu-scheme.md): у места один дом. Здесь — внешние сервисы.
                    ?.filter(({ manifest }) => manifest.kind !== 'mcp' && manifest.setup !== 'brain' && manifest.setup !== 'voice' && !(onComputer() && WITH_STATION.has(manifest.id)))
                    .map((view) => {
                        const { manifest, status } = view;
                        const action = go(view);
                        const where =
                            manifest.setup === 'brain' ? tr.connectors.inBrain : manifest.setup === 'voice' ? tr.connectors.inVoice : '';
                        const body = (
                            <>
                                <ConnectorIcon id={manifest.id} kind={manifest.kind} />
                                <span class="flex-1 min-w-0 flex flex-col gap-2">
                                    <b class="flex gap-8 items-center flex-wrap">
                                        {manifest.title}
                                        {manifest.cloud && status.state !== 'unset' && <span class="pill pill-yandex">{tr.connectors.viaInternet}</span>}
                                    </b>
                                    <span class="t-caption">
                                        {status.detail ?? (onComputer() ? (withoutStation()[manifest.id] ?? manifest.description) : manifest.description)}
                                        {where}
                                    </span>
                                </span>
                                <span class={`pill ${statePill()[status.state].cls}`}>{statePill()[status.state].label}</span>
                            </>
                        );
                        const row = 'flex items-center gap-12 w-full py-10 px-0 bt-divider';
                        return action ? (
                            <button key={manifest.id} class={`dash-option ${row} cursor-pointer`} onClick={action}>
                                {body}
                            </button>
                        ) : (
                            <div key={manifest.id} class={row}>
                                {body}
                            </div>
                        );
                    })}
            </section>
            {open && (
                <ConnectorSheet
                    id={open}
                    kind={opened?.manifest.kind ?? ''}
                    title={opened?.manifest.title}
                    onClose={() => setOpen(null)}
                    onChanged={() => void read()}
                />
            )}
        </>
    );
}

/**
 * Окно подключения: новости — своё, остальные — поля, секреты маской. Одно окно с первого кадра: пока настройки
 * читаются — скелет на их месте, окно не открывается второй раз.
 */
function ConnectorSheet({ id, kind, title, onClose, onChanged }: { id: string; kind: string; title?: string; onClose: () => void; onChanged: () => void }) {
    const [view, setView] = useState<ConnectorSettings | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        api.connectorSettings(id)
            .then(setView)
            .catch((e) => setError(problemOf(e)));
    }, [id]);
    if (id === 'news') return <NewsSheet onClose={onClose} onChanged={onChanged} />;
    return (
        <SettingsForm
            id={id}
            view={view}
            loadError={error}
            title={title}
            kind={kind}
            onClose={onClose}
            onSaved={(fresh) => {
                setView(fresh);
                onChanged();
            }}
        />
    );
}

/** Поля каждого подключения: обычные — их имя в шлюзе и подпись; секреты — отдельно. */
interface FormSpec {
    about: string;
    fields: Array<{ name: string; label: string; placeholder?: string; hint?: preact.ComponentChildren; numeric?: boolean }>;
    secrets: Array<{ name: string; label: string; hint?: preact.ComponentChildren }>;
    contacts?: { title: string; targetLabel: string; targetPlaceholder: string; targetMode: 'numeric' | 'tel' };
    /** Есть переключатель «включено» (у погоды нет: она работает всегда). */
    switchable: boolean;
}

/**
 * Подключения, которым нужна колонка: сессия с Яндексом и звонки на колонку. В режиме «Микрофон»
 * (docs/plans/mic-mode.md) колонки нет — их нет в списке.
 */
const WITH_STATION = new Set(['yandex', 'livekit', 'sip']);

/** Что о подключении сказать без колонки: шлюз описывает его для колонки. */
const withoutStation = (): Record<string, string> => ({
    telegram: tr.connectors.withoutStationTelegram,
});

function specOf(id: string, values: Record<string, unknown>): FormSpec {
    const homeUrl = typeof values.url === 'string' && values.url ? values.url : null;
    switch (id) {
        case 'home-assistant':
            return {
                about: byMode(tr.connectors.haAboutSpeaker, tr.connectors.haAboutOwn),
                fields: [{ name: 'url', label: tr.connectors.haUrl, placeholder: 'http://homeassistant.local:8123' }],
                secrets: [
                    {
                        name: 'token',
                        label: tr.connectors.haToken,
                        hint: (
                            <>
                                {tr.connectors.haTokenHint}
                                {homeUrl && (
                                    <>
                                        {' '}
                                        <a href={`${homeUrl.replace(/\/+$/, '')}/profile/security`} target="_blank" rel="noreferrer">
                                            {tr.connectors.open}
                                        </a>
                                    </>
                                )}
                            </>
                        ),
                    },
                ],
                switchable: true,
            };
        case 'telegram':
            return {
                about: byMode(tr.connectors.tgAboutSpeaker, tr.connectors.tgAboutOwn),
                fields: [],
                secrets: [
                    {
                        name: 'token',
                        label: tr.connectors.tgToken,
                        hint: (
                            <>
                                {tr.connectors.tgTokenHintBefore}{' '}
                                <a href="https://t.me/BotFather" target="_blank" rel="noreferrer">
                                    @BotFather
                                </a>{' '}
                                {tr.connectors.tgTokenHintAfter}
                            </>
                        ),
                    },
                ],
                contacts: { title: byMode(tr.connectors.tgContactsSpeaker, tr.connectors.tgContactsOwn), targetLabel: tr.connectors.tgTargetLabel, targetPlaceholder: tr.connectors.tgTargetPlaceholder, targetMode: 'numeric' },
                switchable: true,
            };
        case 'livekit':
            return {
                about: tr.connectors.lkAbout,
                fields: [
                    { name: 'url', label: tr.connectors.lkUrl, placeholder: 'ws://127.0.0.1:7880' },
                    { name: 'publicUrl', label: tr.connectors.lkPublicUrl, placeholder: 'wss://livekit.example.com', hint: tr.connectors.lkPublicUrlHint },
                    {
                        name: 'pageUrl',
                        label: tr.connectors.lkPageUrl,
                        placeholder: 'https://call.example.com/calls/livekit',
                        hint: tr.connectors.lkPageUrlHint,
                    },
                    { name: 'room', label: tr.connectors.lkRoom, placeholder: 'home' },
                ],
                secrets: [
                    { name: 'apiKey', label: tr.connectors.lkApiKey, hint: tr.connectors.lkApiKeyHint },
                    { name: 'apiSecret', label: tr.connectors.lkApiSecret },
                ],
                switchable: true,
            };
        case 'sip':
            return {
                about: tr.connectors.sipAbout,
                fields: [
                    { name: 'url', label: tr.connectors.sipUrl, placeholder: 'http://127.0.0.1:8088' },
                    { name: 'user', label: tr.connectors.sipUser, placeholder: 'duowave' },
                    { name: 'app', label: tr.connectors.sipApp, placeholder: 'duowave' },
                    { name: 'rtpHost', label: tr.connectors.sipRtpHost, placeholder: '127.0.0.1' },
                    { name: 'rtpPort', label: tr.connectors.sipRtpPort, placeholder: '40000', numeric: true },
                ],
                secrets: [{ name: 'password', label: tr.connectors.sipPassword, hint: tr.connectors.sipPasswordHint }],
                contacts: { title: tr.connectors.sipContacts, targetLabel: tr.connectors.sipTargetLabel, targetPlaceholder: tr.connectors.sipTargetPlaceholder, targetMode: 'tel' },
                switchable: true,
            };
        default:
            return {
                about: byMode(tr.connectors.weatherAboutSpeaker, tr.connectors.weatherAboutOwn),
                fields: [{ name: 'location', label: tr.connectors.weatherCity, placeholder: byMode(tr.connectors.weatherCityPlaceholderSpeaker, tr.connectors.weatherCityPlaceholderOwn) }],
                secrets: [],
                switchable: false,
            };
    }
}

const text = (value: unknown): string => (value === null || value === undefined ? '' : String(value));

/** Выбор «как есть» для чата оповещений, которого нет среди контактов. */
const KEEP = '\u0000keep';

function contactsOf(values: Record<string, unknown>): ContactDraft[] {
    const list = Array.isArray(values.contacts) ? (values.contacts as ContactView[]) : [];
    return list.map((contact, index) => ({ key: `saved-${index}`, name: contact.name, was: contact.name, hint: contact.hint, to: '' }));
}

function SettingsForm({
    id,
    view,
    loadError,
    title,
    kind,
    onClose,
    onSaved,
}: {
    id: string;
    view: ConnectorSettings | null;
    loadError: string | null;
    title?: string;
    kind: string;
    onClose: () => void;
    onSaved: (view: ConnectorSettings) => void;
}) {
    const values = view?.values ?? {};
    const spec = specOf(id, values);
    const fieldsOf = () => Object.fromEntries(spec.fields.map((field) => [field.name, text(values[field.name])]));
    const [fields, setFields] = useState<Record<string, string>>(fieldsOf);
    const [secrets, setSecrets] = useState<Record<string, string>>({});
    const [removing, setRemoving] = useState<Record<string, boolean>>({});
    const [contacts, setContacts] = useState<ContactDraft[]>(() => contactsOf(values));
    // Чат оповещений не из контактов (перенесён из .env) остаётся, пока не выбран другой.
    const alertOf = () => (typeof values.alertHint === 'string' ? KEEP : text(values.alertTo));
    const [alertTo, setAlertTo] = useState(alertOf);
    const [strangers, setStrangers] = useState<TelegramStranger[]>([]);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const reset = () => {
        setFields(fieldsOf());
        setSecrets({});
        setRemoving({});
        setContacts(contactsOf(values));
        setAlertTo(alertOf());
    };
    // Настройки пришли — поля встают в их значения (окно то же, без второго открытия).
    const loaded = view !== null;
    useEffect(() => {
        if (loaded) reset();
    }, [loaded]);

    useEffect(() => {
        if (id === 'telegram' && view?.available)
            api.telegramChats()
                .then(setStrangers)
                .catch(() => setStrangers([]));
    }, [id, view?.available]);

    if (!view) {
        return (
            <Sheet title={title ?? tr.connectors.sheetTitle} icon={<ConnectorIcon id={id} kind={kind} />} onClose={onClose}>
                {loadError ? (
                    <p class="win-callout m-0" role="alert">
                        {loadError}
                    </p>
                ) : (
                    <>
                        <div class="flex flex-col gap-8" aria-hidden="true">
                            <span class="sk sk-line" style={{ width: '92%' }} />
                            <span class="sk sk-line" style={{ width: '64%' }} />
                        </div>
                        <SkeletonRows rows={2} label={tr.connectors.readingSettings} />
                    </>
                )}
            </Sheet>
        );
    }

    // Что изменилось: шлюзу уходит только это. «Включено» — сразу, отдельно (переключатель полосы не ждёт).
    const change: Record<string, unknown> = {};
    for (const field of spec.fields) {
        const before = text(view.values[field.name]);
        if (fields[field.name] !== before) change[field.name] = field.numeric ? Number(fields[field.name]) : fields[field.name];
    }
    for (const secret of spec.secrets) {
        if (removing[secret.name]) change[secret.name] = '';
        else if ((secrets[secret.name] ?? '').trim()) change[secret.name] = secrets[secret.name].trim();
    }
    if (spec.contacts) {
        const before = JSON.stringify(contactsInput(contactsOf(view.values)));
        const now = contactsInput(contacts);
        if (JSON.stringify(now) !== before) change.contacts = now;
    }
    if (id === 'telegram' && alertTo !== KEEP && alertTo !== text(view.values.alertTo)) change.alertTo = alertTo;
    const changed = Object.keys(change).length > 0;

    const act = async (key: string, work: () => Promise<ConnectorSettings>) => {
        setBusy(key);
        setError(null);
        try {
            const fresh = await work();
            onSaved(fresh);
            return true;
        } catch (e) {
            setError(problemOf(e));
            return false;
        } finally {
            setBusy(null);
        }
    };
    const save = () =>
        void act('save', async () => {
            let fresh = changed ? await api.updateConnector(view.id, change) : view;
            if (fresh.status.state !== 'unset' && fresh.status.state !== 'off') fresh = await api.checkConnector(view.id);
            return fresh;
        }).then((ok) => {
            if (ok) {
                setSecrets({});
                setRemoving({});
            }
        });
    const enabled = view.values.enabled !== false;
    const flip = (on: boolean) => void act('enabled', () => api.updateConnector(view.id, { enabled: on }));
    const clear = async () => {
        const yes = await ask({
            title: tr.connectors.clearTitle(view.title),
            text: tr.connectors.clearText,
            ok: tr.connectors.clear,
            danger: true,
        });
        if (!yes) return;
        void act('clear', () => api.clearConnector(view.id));
    };
    const ready = view.status.state !== 'unset' && view.status.state !== 'off';
    const pill = statePill()[view.status.state];
    const named = contacts.filter((contact) => contact.name.trim());
    const known = new Set(contacts.map((contact) => contact.stranger).filter(Boolean));
    const fresh = strangers.filter((stranger) => !known.has(stranger.id));

    // Нижняя полоса: изменили — «Сохранить и проверить»; нет — «Проверить», если есть что проверять.
    const footer = !view.available ? undefined : changed || busy === 'save' ? (
        <SaveBar dirty={changed} busy={busy === 'save'} saveLabel={tr.connectors.andCheck(saveLabel())} onRevert={reset} onSave={save} />
    ) : ready ? (
        <button class="btn btn-plain" disabled={busy !== null} aria-busy={busy === 'save' || undefined} onClick={save}>
            {tr.connectors.check}
        </button>
    ) : undefined;

    return (
        <Sheet
            title={view.title}
            meta={
                <>
                    <span class={`pill ${pill.cls}`}>{pill.label}</span>
                    {view.status.detail && <span class="win-chip">{view.status.detail}</span>}
                </>
            }
            icon={<ConnectorIcon id={view.id} kind={kind} />}
            onClose={onClose}
            footer={footer}
        >
            <p class="win-lead">{spec.about}</p>
            {!view.available ? (
                <p class="win-callout is-calm m-0">{tr.connectors.noCallsOnStation}</p>
            ) : (
                <>
                    <EnvNote fromEnv={view.fromEnv} envLeft={view.envLeft} />
                    <CheckLine check={view.check} />
                    {view.busy && (
                        <p class="win-callout m-0" role="status">
                            {tr.connectors.callInProgress}
                        </p>
                    )}
                    {spec.switchable && (
                        <WinSection group>
                            <WinRow title={tr.connectors.enabled} sub={tr.connectors.enabledHint}>
                                <Switch on={enabled} label={tr.connectors.enabled} disabled={busy !== null} onChange={flip} />
                            </WinRow>
                        </WinSection>
                    )}
                    {(spec.fields.length > 0 || spec.secrets.length > 0) && (
                        <section class="win-sec gap-14">
                            {spec.fields.map((field) => (
                                <TextField
                                    key={field.name}
                                    label={field.label}
                                    value={fields[field.name] ?? ''}
                                    placeholder={field.placeholder}
                                    hint={field.hint}
                                    inputMode={field.numeric ? 'numeric' : 'url'}
                                    onInput={(value) => setFields((all) => ({ ...all, [field.name]: value }))}
                                />
                            ))}
                            {spec.secrets.map((secret) => (
                                <SecretField
                                    key={secret.name}
                                    label={secret.label}
                                    mask={view.secrets[secret.name]}
                                    value={secrets[secret.name] ?? ''}
                                    removing={!!removing[secret.name]}
                                    hint={secret.hint}
                                    onInput={(value) => setSecrets((all) => ({ ...all, [secret.name]: value }))}
                                    onRemove={(on) => setRemoving((all) => ({ ...all, [secret.name]: on }))}
                                />
                            ))}
                            {spec.secrets.length > 0 && <p class="win-note">{tr.connectors.secretsNote}</p>}
                        </section>
                    )}
                    {spec.contacts && (
                        <section class="win-sec">
                            <ContactsField
                                title={spec.contacts.title}
                                contacts={contacts}
                                targetLabel={spec.contacts.targetLabel}
                                targetPlaceholder={spec.contacts.targetPlaceholder}
                                targetMode={spec.contacts.targetMode}
                                onChange={setContacts}
                            />
                        </section>
                    )}
                    {id === 'telegram' && fresh.length > 0 && (
                        <WinSection title={byMode(tr.connectors.wroteToBotSpeaker, tr.connectors.wroteToBotOwn)} group>
                            {fresh.map((stranger) => (
                                <WinRow key={stranger.id} title={stranger.name} sub={tr.connectors.chatWithAgo(stranger.hint, timeAgo(stranger.at))}>
                                    <button
                                        class="btn btn-plain btn-sm"
                                        onClick={() =>
                                            setContacts((all) => [
                                                ...all,
                                                { key: `stranger-${stranger.id}`, name: stranger.name.split(' ')[0], hint: stranger.hint, to: '', stranger: stranger.id },
                                            ])
                                        }
                                    >
                                        {tr.connectors.add}
                                    </button>
                                </WinRow>
                            ))}
                        </WinSection>
                    )}
                    {id === 'telegram' && (
                        <WinSection group>
                            <div class="win-row is-col">
                                <span class="win-row-t">{byMode(tr.connectors.alertsSpeaker, tr.connectors.alertsOwn)}</span>
                                <select class="field" value={alertTo} onChange={(e) => setAlertTo((e.target as HTMLSelectElement).value)}>
                                    {typeof view.values.alertHint === 'string' && <option value={KEEP}>{tr.connectors.chatNotFromContacts(view.values.alertHint)}</option>}
                                    <option value="">{tr.connectors.nowhere}</option>
                                    {named.map((contact) => (
                                        <option key={contact.key} value={contact.name.trim()}>
                                            {contact.name.trim()}
                                        </option>
                                    ))}
                                </select>
                            </div>
                        </WinSection>
                    )}
                    {view.stored && (
                        <WinSection title={tr.ui.danger} group>
                            <WinRow title={tr.connectors.clearSettings} sub={tr.connectors.clearText}>
                                <button class="btn btn-danger btn-sm" disabled={busy !== null} aria-busy={busy === 'clear' || undefined} onClick={() => void clear()}>
                                    {tr.connectors.clear}
                                </button>
                            </WinRow>
                        </WinSection>
                    )}
                </>
            )}
            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
