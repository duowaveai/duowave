import type { ComponentChildren } from 'preact';

import type { ConnectorCheck } from '../api';
import { timeAgo } from '../time';
import { Action } from './cards';

/** Поле с подписью над ним и подсказкой под ним. */
export function TextField({
    label,
    value,
    placeholder,
    hint,
    type = 'text',
    inputMode,
    onInput,
}: {
    label: string;
    value: string;
    placeholder?: string;
    hint?: ComponentChildren;
    type?: 'text' | 'url' | 'number';
    inputMode?: 'text' | 'url' | 'numeric' | 'tel';
    onInput: (value: string) => void;
}) {
    return (
        <label class="flex flex-col gap-4">
            <span class="t-caption">{label}</span>
            <input
                class="field"
                type={type}
                inputMode={inputMode}
                autocomplete="off"
                spellcheck={false}
                placeholder={placeholder}
                value={value}
                onInput={(e) => onInput((e.target as HTMLInputElement).value)}
            />
            {hint && <span class="t-meta">{hint}</span>}
        </label>
    );
}

/**
 * Секрет: что задано — только маской «задан · …a1b2», новое значение вписывают заново. Пустое поле ничего не
 * меняет; «убрать» стирает секрет при сохранении.
 */
export function SecretField({
    label,
    mask,
    value,
    removing,
    placeholder,
    hint,
    onInput,
    onRemove,
}: {
    label: string;
    mask?: { set: boolean; hint?: string };
    value: string;
    /** Отмечено «убрать»: сохранение сотрёт секрет. */
    removing: boolean;
    placeholder?: string;
    hint?: ComponentChildren;
    onInput: (value: string) => void;
    onRemove: (removing: boolean) => void;
}) {
    const state = removing ? 'будет убран' : mask?.set ? `задан · ${mask.hint ?? ''}` : 'не задан';
    return (
        <label class="flex flex-col gap-4">
            <span class="t-caption flex gap-8 items-center flex-wrap">
                {label}
                <span class={`pill ${mask?.set && !removing ? 'pill-own' : 'pill-off'} py-2 px-8`}>
                    {state}
                </span>
            </span>
            <span class="flex gap-8 items-center">
                <input
                    class="field flex-1 min-w-0"
                    type="password"
                    autocomplete="off"
                    spellcheck={false}
                    placeholder={placeholder ?? (mask?.set ? 'новое значение' : 'вставьте сюда')}
                    value={value}
                    disabled={removing}
                    onInput={(e) => onInput((e.target as HTMLInputElement).value)}
                />
                {mask?.set && (
                    <Action label={removing ? 'оставить' : 'убрать'} onClick={() => onRemove(!removing)} />
                )}
            </span>
            {hint && <span class="t-meta">{hint}</span>}
        </label>
    );
}

/** Контакт в листе: заданный раньше (его номер виден последними цифрами), новый или из «Написали боту». */
export interface ContactDraft {
    /** Стабильный ключ строки. */
    key: string;
    name: string;
    /** Имя, под которым контакт сохранён: его номер шлюз сохранит. */
    was?: string;
    hint?: string;
    /** Новый номер; пустой — оставить прежний. */
    to: string;
    /** Из «Написали боту»: номер чата знает только шлюз. */
    stranger?: string;
}

/** Контакты: имя, которое говорят колонке, и куда он ведёт. Номер уже заданного не показывается — только хвост. */
export function ContactsField({
    title,
    contacts,
    targetLabel,
    targetPlaceholder,
    targetMode,
    onChange,
}: {
    title: string;
    contacts: ContactDraft[];
    targetLabel: string;
    targetPlaceholder: string;
    targetMode: 'numeric' | 'tel' | 'text';
    onChange: (contacts: ContactDraft[]) => void;
}) {
    const edit = (key: string, change: Partial<ContactDraft>) =>
        onChange(contacts.map((contact) => (contact.key === key ? { ...contact, ...change } : contact)));
    return (
        <div class="flex flex-col gap-6">
            <span class="t-caption">{title}</span>
            {contacts.length === 0 && (
                <span class="t-meta">Пока никого. Имя — как его скажут колонке: «передай маме…» найдёт «мама».</span>
            )}
            {contacts.map((contact) => (
                <div key={contact.key} class="grid grid-pair-36 gap-6 items-center">
                    <input
                        class="field min-w-0"
                        aria-label="Имя"
                        placeholder="имя, например мама"
                        value={contact.name}
                        onInput={(e) => edit(contact.key, { name: (e.target as HTMLInputElement).value })}
                    />
                    {contact.stranger ? (
                        <span class="t-caption">написал боту · {contact.hint}</span>
                    ) : (
                        <input
                            class="field min-w-0"
                            aria-label={contact.hint ? `${targetLabel} ${contact.name}: новый вместо ${contact.hint}` : `${targetLabel} ${contact.name}`}
                            inputMode={targetMode}
                            autocomplete="off"
                            placeholder={contact.hint ? `${targetLabel} ${contact.hint}` : targetPlaceholder}
                            value={contact.to}
                            onInput={(e) => edit(contact.key, { to: (e.target as HTMLInputElement).value })}
                        />
                    )}
                    <button
                        type="button"
                        class="pill pill-off cursor-pointer w-36 h-36 p-0 inline-flex items-center justify-center fs-18"
                        aria-label={`Убрать контакт ${contact.name}`}
                        title="Убрать"
                        onClick={() => onChange(contacts.filter((other) => other.key !== contact.key))}
                    >
                        ×
                    </button>
                </div>
            ))}
            {contacts.some((contact) => contact.hint && !contact.stranger) && (
                <span class="t-meta">Номер уже заданного виден последними цифрами; впишите новый, чтобы заменить.</span>
            )}
            <span>
                <Action
                    label="добавить контакт"
                    onClick={() => onChange([...contacts, { key: `new-${Date.now()}`, name: '', to: '' }])}
                />
            </span>
        </div>
    );
}

/** Контакты листа в том виде, что ждёт шлюз: имя и новый номер, имя прежнего — или чат написавшего боту. */
export function contactsInput(contacts: ContactDraft[]): Array<Record<string, string>> {
    return contacts
        .filter((contact) => contact.name.trim())
        .map((contact) => {
            const entry: Record<string, string> = { name: contact.name.trim() };
            if (contact.stranger) entry.stranger = contact.stranger;
            else if (contact.to.trim()) entry.to = contact.to.trim();
            else if (contact.was) entry.was = contact.was;
            return entry;
        });
}

/** Итог «Проверить»: лайм — работает, коралл — нет, и когда проверяли. */
export function CheckLine({ check }: { check?: ConnectorCheck | null }) {
    if (!check) return null;
    return (
        <p
            class={`t-meta m-0 flex gap-8 items-center ${check.ok ? 'c-accent-text' : 'c-dot-offline'}`}
            role="status"
        >
            <span aria-hidden="true">{check.ok ? '●' : '○'}</span>
            <span>
                {check.ok ? 'Проверено' : 'Не работает'}: {check.text}
                <span class="t-caption ml-6">
                    {timeAgo(check.at)}
                </span>
            </span>
        </p>
    );
}

/** Откуда настройки: перенесены из .env — и какие строки .env шлюз больше не читает. */
export function EnvNote({ fromEnv, envLeft }: { fromEnv: boolean; envLeft: string[] }) {
    if (!fromEnv && envLeft.length === 0) return null;
    return (
        <p class="t-meta m-0">
            {fromEnv ? 'Перенесено из .env при обновлении шлюза; дальше меняется здесь.' : 'Настроено здесь.'}
            {envLeft.length > 0 && (
                <>
                    {' '}
                    Шлюз больше не читает из .env: {envLeft.join(', ')} — {envLeft.length === 1 ? 'эту строку' : 'эти строки'} можно убрать.
                </>
            )}
        </p>
    );
}
