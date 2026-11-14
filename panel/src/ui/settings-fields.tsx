// Поля настроек приложения (docs/plans/voice-creation.md, «Что ему нужно»): одни и те же в карточке установленного
// (screens/plugins.tsx) и в «Примерке» черновика (screens/fitting.tsx). Строка (или выбор), число, да/нет, личное.
// Личное (`secret`) шлюз не отдаёт: заданное видно маской, пустое поле уходит маской — значение остаётся прежним. Пока
// поле личного открыто, микрофон пульта выключен (`onPrivate`).

import { Switch } from './switch';
import { tr } from '../i18n';

/** Маска секрета (ссылка с ключом внутри): значение шлюз не отдаёт, а та же маска при сохранении оставляет прежнее. */
export const SECRET_MASK = '••••••';

/** Настройка, как её видит пульт: из карточки пакета или из того, что нужно черновику. */
export interface SettingFieldSpec {
    key: string;
    title: string;
    type: string;
    description?: string | null;
    options?: string[] | null;
    default?: unknown;
    secret?: boolean;
    /** Что задано сейчас (личное — маской). */
    value?: unknown;
}

export const isSecret = (field: SettingFieldSpec): boolean => field.type === 'secret' || field.secret === true || field.value === SECRET_MASK;

/** Поле одной настройки. `onPrivate(true)` — поле личного открыто, `false` — закрыто. */
export function SettingInput({
    field,
    value,
    busy,
    onChange,
    onPrivate,
}: {
    field: SettingFieldSpec;
    value: unknown;
    busy?: boolean;
    onChange: (value: unknown) => void;
    onPrivate?: (open: boolean) => void;
}) {
    if (field.type === 'boolean') return <Switch on={value === true} label={field.title} disabled={busy} onChange={(on) => onChange(on)} />;
    if (isSecret(field))
        return (
            // Заданный секрет не показывается: поле пустое, и пустым оно уходит маской — шлюз оставит прежнее.
            <input
                class="field"
                type="password"
                autocomplete="off"
                spellcheck={false}
                value={value === SECRET_MASK ? '' : String(value ?? '')}
                placeholder={field.value === SECRET_MASK ? tr.ui.secretSetReplace : tr.ui.pasteHere}
                disabled={busy}
                onFocus={() => onPrivate?.(true)}
                onBlur={() => onPrivate?.(false)}
                onInput={(e) => {
                    const text = (e.target as HTMLInputElement).value;
                    onChange(text === '' && field.value === SECRET_MASK ? SECRET_MASK : text);
                }}
            />
        );
    if (field.options)
        return (
            <select class="field" value={String(value ?? '')} disabled={busy} onChange={(e) => onChange((e.target as HTMLSelectElement).value)}>
                {field.options.map((option) => (
                    <option key={option} value={option}>
                        {option}
                    </option>
                ))}
            </select>
        );
    return (
        <input
            class="field"
            type={field.type === 'number' ? 'number' : 'text'}
            value={value == null ? '' : String(value)}
            disabled={busy}
            onInput={(e) => {
                const text = (e.target as HTMLInputElement).value;
                onChange(field.type === 'number' ? (text === '' ? null : Number(text)) : text);
            }}
        />
    );
}

/** Поля настроек подряд, с подписями. */
export function SettingsFields({
    fields,
    values,
    busy,
    onChange,
    onPrivate,
}: {
    fields: SettingFieldSpec[];
    values: Record<string, unknown>;
    busy?: boolean;
    onChange: (values: Record<string, unknown>) => void;
    onPrivate?: (open: boolean) => void;
}) {
    return (
        <>
            {fields.map((field) => (
                <label key={field.key} class="flex flex-col gap-4">
                    <span class="fw-700">{field.title}</span>
                    {field.description && <span class="t-caption">{field.description}</span>}
                    <SettingInput field={field} value={values[field.key]} busy={busy} onChange={(value) => onChange({ ...values, [field.key]: value })} onPrivate={onPrivate} />
                </label>
            ))}
        </>
    );
}
