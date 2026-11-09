import { useEffect, useState } from 'preact/hooks';

import { ConnectorSettings, ConnectorView, ContactView, TelegramStranger, api, can, problemOf } from '../api';
import { type BrandId } from '../brand-icons';
import { IconEar, IconGlobe, IconHome, IconLinks, IconMcp, IconPhone, IconRobot, IconSend, IconSparkle, IconSpeaker, IconVoice } from '../icons';
import { timeAgo } from '../time';
import { BrandMark } from '../ui/brand';
import { Action } from '../ui/cards';
import { CheckLine, ContactDraft, ContactsField, EnvNote, SecretField, TextField, contactsInput } from '../ui/fields';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import type { SettingsTab } from './mind';
import { NewsSheet } from './news';
import { ask } from '../ui/confirm';

/** Работает и настроено — лайм, не настроено и выключено — сланец, нет связи — коралл, «скоро» — чего в этой сборке ещё нет. */
const STATE_PILL: Record<ConnectorView['status']['state'], { cls: string; label: string }> = {
    ready: { cls: 'pill-own', label: 'работает' },
    configured: { cls: 'pill-own', label: 'настроено' },
    unset: { cls: 'pill-off', label: 'не настроено' },
    off: { cls: 'pill-off', label: 'выключено' },
    soon: { cls: 'pill-off', label: 'скоро' },
    error: { cls: 'pill-error', label: 'нет связи' },
};

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
                        Подключения
                    </h2>
                    <span class="t-meta">{mayManage ? 'нажмите, чтобы настроить' : 'MCP-серверы — ниже'}</span>
                </div>
                {error && (
                    <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                        {error}
                    </p>
                )}
                {!list && !error && (
                    <p class="t-meta m-0 mb-8">
                        Спрашиваем шлюз…
                    </p>
                )}
                {list
                    ?.filter(({ manifest }) => manifest.kind !== 'mcp')
                    .map((view) => {
                        const { manifest, status } = view;
                        const action = go(view);
                        const where =
                            manifest.setup === 'brain' ? ' · в «Мозге»' : manifest.setup === 'voice' ? ' · в «Голосе»' : '';
                        const body = (
                            <>
                                <ConnectorIcon id={manifest.id} kind={manifest.kind} />
                                <span class="flex-1 min-w-0 flex flex-col gap-2">
                                    <b class="flex gap-8 items-center flex-wrap">
                                        {manifest.title}
                                        {manifest.cloud && status.state !== 'unset' && <span class="pill pill-yandex">через интернет</span>}
                                    </b>
                                    <span class="t-caption">
                                        {status.detail ?? manifest.description}
                                        {where}
                                    </span>
                                </span>
                                <span class={`pill ${STATE_PILL[status.state].cls}`}>{STATE_PILL[status.state].label}</span>
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
                    onClose={() => setOpen(null)}
                    onChanged={() => void read()}
                />
            )}
        </>
    );
}

/** Лист подключения: новости — свой, остальные — поля, секреты маской, «Сохранить и проверить», «Очистить». */
function ConnectorSheet({ id, kind, onClose, onChanged }: { id: string; kind: string; onClose: () => void; onChanged: () => void }) {
    const [view, setView] = useState<ConnectorSettings | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        api.connectorSettings(id)
            .then(setView)
            .catch((e) => setError(problemOf(e)));
    }, [id]);
    if (id === 'news') return <NewsSheet onClose={onClose} onChanged={onChanged} />;
    if (!view)
        return (
            <Sheet title="Подключение" icon={<ConnectorIcon id={id} kind={kind} />} onClose={onClose}>
                <p class={`t-meta m-0${error ? ' c-dot-offline' : ''}`} role={error ? 'alert' : undefined}>
                    {error ?? 'Читаем настройки…'}
                </p>
            </Sheet>
        );
    return (
        <SettingsForm
            key={view.id}
            view={view}
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

function specOf(id: string, values: Record<string, unknown>): FormSpec {
    const homeUrl = typeof values.url === 'string' && values.url ? values.url : null;
    switch (id) {
        case 'home-assistant':
            return {
                about: 'Свет, розетки, климат и сценарии дома. Колонка говорит с Home Assistant напрямую, по домашней сети.',
                fields: [{ name: 'url', label: 'Адрес Home Assistant дома', placeholder: 'http://homeassistant.local:8123' }],
                secrets: [
                    {
                        name: 'token',
                        label: 'Долгосрочный токен',
                        hint: (
                            <>
                                В Home Assistant: профиль → «Безопасность» → «Долгосрочные токены доступа» → «Создать токен».
                                {homeUrl && (
                                    <>
                                        {' '}
                                        <a href={`${homeUrl.replace(/\/+$/, '')}/profile/security`} target="_blank" rel="noreferrer">
                                            Открыть
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
                about: 'Голосовые сообщения семье через своего бота, ссылки на звонки и оповещения о колонке. Сообщения уходят через Telegram.',
                fields: [],
                secrets: [
                    {
                        name: 'token',
                        label: 'Токен бота',
                        hint: (
                            <>
                                Его выдаёт{' '}
                                <a href="https://t.me/BotFather" target="_blank" rel="noreferrer">
                                    @BotFather
                                </a>{' '}
                                в Telegram: команда /newbot.
                            </>
                        ),
                    },
                ],
                contacts: { title: 'Контакты: кому колонка передаёт и чьи сообщения слышит', targetLabel: 'чат', targetPlaceholder: 'номер чата', targetMode: 'numeric' },
                switchable: true,
            };
        case 'livekit':
            return {
                about: 'Звонки на колонку из браузера и приложения, дома и вне дома. Свой сервер LiveKit: deploy/livekit.',
                fields: [
                    { name: 'url', label: 'Адрес сервера для шлюза', placeholder: 'ws://127.0.0.1:7880' },
                    { name: 'publicUrl', label: 'Адрес для телефонов вне дома', placeholder: 'wss://livekit.example.com', hint: 'Пусто — только дома.' },
                    {
                        name: 'pageUrl',
                        label: 'Страница звонка',
                        placeholder: 'https://call.example.com/calls/livekit',
                        hint: 'По ней колонка зовёт контакт из Telegram («позвони маме»). Пусто — без ссылок.',
                    },
                    { name: 'room', label: 'Комната', placeholder: 'home' },
                ],
                secrets: [
                    { name: 'apiKey', label: 'API-ключ', hint: 'Ключ и секрет — из keys в livekit.yaml сервера.' },
                    { name: 'apiSecret', label: 'API-секрет' },
                ],
                switchable: true,
            };
        case 'sip':
            return {
                about: 'Звонки на обычные телефоны через свой Asterisk (deploy/asterisk): звонок с телефона звонит на колонку, «позвони маме» набирает номер.',
                fields: [
                    { name: 'url', label: 'Адрес ARI', placeholder: 'http://127.0.0.1:8088' },
                    { name: 'user', label: 'Логин ARI', placeholder: 'duowave' },
                    { name: 'app', label: 'Приложение Stasis', placeholder: 'duowave' },
                    { name: 'rtpHost', label: 'Адрес шлюза для звука (RTP)', placeholder: '127.0.0.1' },
                    { name: 'rtpPort', label: 'Порт RTP', placeholder: '40000', numeric: true },
                ],
                secrets: [{ name: 'password', label: 'Пароль ARI', hint: 'Из ari.conf Asterisk.' }],
                contacts: { title: 'Контакты: номера и внутренние', targetLabel: 'номер', targetPlaceholder: '+7… или 101', targetMode: 'tel' },
                switchable: true,
            };
        default:
            return {
                about: 'Прогноз Open-Meteo без ключа. Город, о котором колонка говорит, если в вопросе города нет.',
                fields: [{ name: 'location', label: 'Город', placeholder: 'пусто — по часовому поясу колонки' }],
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

function SettingsForm({ view, kind, onClose, onSaved }: { view: ConnectorSettings; kind: string; onClose: () => void; onSaved: (view: ConnectorSettings) => void }) {
    const spec = specOf(view.id, view.values);
    const [fields, setFields] = useState<Record<string, string>>(() =>
        Object.fromEntries(spec.fields.map((field) => [field.name, text(view.values[field.name])])),
    );
    const [enabled, setEnabled] = useState(view.values.enabled !== false);
    const [secrets, setSecrets] = useState<Record<string, string>>({});
    const [removing, setRemoving] = useState<Record<string, boolean>>({});
    const [contacts, setContacts] = useState<ContactDraft[]>(() => contactsOf(view.values));
    // Чат оповещений не из контактов (перенесён из .env) остаётся, пока не выбран другой.
    const [alertTo, setAlertTo] = useState(typeof view.values.alertHint === 'string' ? KEEP : text(view.values.alertTo));
    const [strangers, setStrangers] = useState<TelegramStranger[]>([]);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (view.id === 'telegram' && view.available)
            api.telegramChats()
                .then(setStrangers)
                .catch(() => setStrangers([]));
    }, [view.id]);

    // Что изменилось: шлюзу уходит только это.
    const change: Record<string, unknown> = {};
    for (const field of spec.fields) {
        const before = text(view.values[field.name]);
        if (fields[field.name] !== before) change[field.name] = field.numeric ? Number(fields[field.name]) : fields[field.name];
    }
    if (spec.switchable && enabled !== (view.values.enabled !== false)) change.enabled = enabled;
    for (const secret of spec.secrets) {
        if (removing[secret.name]) change[secret.name] = '';
        else if ((secrets[secret.name] ?? '').trim()) change[secret.name] = secrets[secret.name].trim();
    }
    if (spec.contacts) {
        const before = JSON.stringify(contactsInput(contactsOf(view.values)));
        const now = contactsInput(contacts);
        if (JSON.stringify(now) !== before) change.contacts = now;
    }
    if (view.id === 'telegram' && alertTo !== KEEP && alertTo !== text(view.values.alertTo)) change.alertTo = alertTo;
    const changed = Object.keys(change).length > 0;

    const act = async (key: string, work: () => Promise<ConnectorSettings>) => {
        setBusy(key);
        setError(null);
        try {
            const fresh = await work();
            onSaved(fresh);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const save = () =>
        void act('save', async () => {
            let fresh = changed ? await api.updateConnector(view.id, change) : view;
            if (fresh.status.state !== 'unset' && fresh.status.state !== 'off') fresh = await api.checkConnector(view.id);
            return fresh;
        });
    const clear = async () => {
        const yes = await ask({
            title: `Очистить настройки «${view.title}»?`,
            text: 'Адреса, ключи и контакты удалятся; .env их не вернёт.',
            ok: 'Очистить',
            danger: true,
        });
        if (!yes) return;
        void act('clear', () => api.clearConnector(view.id));
    };
    const ready = view.status.state !== 'unset' && view.status.state !== 'off';
    const pill = STATE_PILL[view.status.state];
    const named = contacts.filter((contact) => contact.name.trim());
    const known = new Set(contacts.map((contact) => contact.stranger).filter(Boolean));

    return (
        <Sheet
            title={view.title}
            about={view.status.detail ? `${pill.label} · ${view.status.detail}` : pill.label}
            icon={<ConnectorIcon id={view.id} kind={kind} />}
            onClose={onClose}
            footer={
                view.available ? (
                    <div class="flex gap-8 flex-wrap">
                        <button
                            class="pill pill-own dash-confirm-btn"
                            disabled={busy !== null || (!changed && !ready)}
                            aria-busy={busy === 'save'}
                            onClick={save}
                        >
                            {busy === 'save' ? (changed ? 'Сохраняем…' : 'Проверяем…') : changed ? 'Сохранить и проверить' : 'Проверить'}
                        </button>
                        <button class="pill pill-off dash-confirm-btn" onClick={onClose}>
                            Закрыть
                        </button>
                    </div>
                ) : undefined
            }
        >
            <p class="t-meta m-0">
                {spec.about}
            </p>
            {!view.available ? (
                <p class="t-meta m-0">
                    В сборке для колонки звонков нет: Telegram, LiveKit и SIP настраиваются на шлюзе с компьютера.
                </p>
            ) : (
                <>
                    <EnvNote fromEnv={view.fromEnv} envLeft={view.envLeft} />
                    <CheckLine check={view.check} />
                    {view.busy && (
                        <p class="t-meta m-0 c-pill-yandex-fg" role="status">
                            Сейчас идёт звонок через этот мост: сохранение его прервёт.
                        </p>
                    )}
                    {spec.switchable && (
                        <label class="flex items-center justify-between gap-12 min-h-44">
                            <span class="flex flex-col">
                                <b>Включено</b>
                                <span class="t-caption">Выключенное подключение помнит свои настройки.</span>
                            </span>
                            <Switch on={enabled} label="Включено" onChange={setEnabled} />
                        </label>
                    )}
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
                    {spec.contacts && (
                        <ContactsField
                            title={spec.contacts.title}
                            contacts={contacts}
                            targetLabel={spec.contacts.targetLabel}
                            targetPlaceholder={spec.contacts.targetPlaceholder}
                            targetMode={spec.contacts.targetMode}
                            onChange={setContacts}
                        />
                    )}
                    {view.id === 'telegram' && strangers.some((stranger) => !known.has(stranger.id)) && (
                        <div class="flex flex-col gap-6">
                            <span class="t-caption">Написали боту — добавьте, кого колонка должна слышать</span>
                            {strangers
                                .filter((stranger) => !known.has(stranger.id))
                                .map((stranger) => (
                                    <div key={stranger.id} class="flex items-center gap-8 flex-wrap">
                                        <span class="flex-1 min-w-0">
                                            <b>{stranger.name}</b>{' '}
                                            <span class="t-caption">
                                                чат {stranger.hint} · {timeAgo(stranger.at)}
                                            </span>
                                        </span>
                                        <Action
                                            label="добавить"
                                            tone="on"
                                            onClick={() =>
                                                setContacts((all) => [
                                                    ...all,
                                                    { key: `stranger-${stranger.id}`, name: stranger.name.split(' ')[0], hint: stranger.hint, to: '', stranger: stranger.id },
                                                ])
                                            }
                                        />
                                    </div>
                                ))}
                        </div>
                    )}
                    {view.id === 'telegram' && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">Оповещения о колонке («пропала», «снова на связи», новая колонка)</span>
                            <select class="field" value={alertTo} onChange={(e) => setAlertTo((e.target as HTMLSelectElement).value)}>
                                {typeof view.values.alertHint === 'string' && <option value={KEEP}>чат {view.values.alertHint} — не из контактов</option>}
                                <option value="">никуда</option>
                                {named.map((contact) => (
                                    <option key={contact.key} value={contact.name.trim()}>
                                        {contact.name.trim()}
                                    </option>
                                ))}
                            </select>
                        </label>
                    )}
                    <p class="t-meta m-0">
                        Ключи и пароли хранятся в шлюзе зашифрованными; пульт показывает только их последние знаки.
                    </p>
                    {view.stored && (
                        <span>
                            <Action label="очистить настройки" busy={busy === 'clear'} disabled={busy !== null} onClick={clear} />
                        </span>
                    )}
                </>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
