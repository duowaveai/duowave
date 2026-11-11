import type { Ref } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { PersonaInfo, api, problemOf } from '../api';
import { Action, Card, CardNote } from '../ui/cards';

/**
 * Редактор текста характера: поле, подсказка про правила речи и «сохранить». Его же открывает «Тонкая настройка»
 * персонажа и «Собрать своего» (characters.tsx); сохраняет вызывающий — PUT /api/v1/persona.
 */
export function PersonaEditor({
    draft,
    onDraft,
    onSave,
    busy = false,
    basis,
    editorRef,
    label = 'Свой характер',
    placeholder = 'Например: «Ты — Алиса, но отвечаешь как старый морской волк…»',
    saveLabel = 'сохранить',
    readOnly = false,
}: {
    draft: string;
    onDraft: (text: string) => void;
    onSave: () => void;
    busy?: boolean;
    /** Чей текст взят за основу — подпись под полем. */
    basis?: string;
    editorRef?: Ref<HTMLTextAreaElement>;
    label?: string;
    placeholder?: string;
    saveLabel?: string;
    /** Только посмотреть: без «сохранить». */
    readOnly?: boolean;
}) {
    return (
        <>
            <textarea
                ref={editorRef}
                value={draft}
                readOnly={readOnly}
                onInput={(e) => onDraft((e.target as HTMLTextAreaElement).value)}
                placeholder={placeholder}
                aria-label={label}
                class="w-full min-h-96 py-10 px-12 r-12 border-glass bg-icon-bg resize-y"
            />
            <div class="flex items-center gap-10">
                <span class="t-caption flex-1">
                    {basis ? `Основа — «${basis}». ` : ''}Правила речи (коротко, без списков и эмодзи) шлюз добавит сам.
                </span>
                {!readOnly && <Action label={saveLabel} tone="on" busy={busy} onClick={onSave} />}
            </div>
        </>
    );
}

/**
 * Характер Алисы: готовые карточки применяются сразу (промт читается на каждый вопрос), свой — пишется в
 * редакторе и сохраняется. Правила речи (коротко, без списков и эмодзи) шлюз добавляет к любому тексту сам.
 * Шлюз с персонажами показывает «Мозг» → «Персонажи» (characters.tsx); этот вид — для шлюза постарше.
 */
export function PersonaTab({ isAdmin }: { isAdmin: boolean }) {
    const [info, setInfo] = useState<PersonaInfo | null>(null);
    const [draft, setDraft] = useState('');
    const [edited, setEdited] = useState(false);
    const [basis, setBasis] = useState('');
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const [busy, setBusy] = useState<Record<string, boolean>>({});
    const editor = useRef<HTMLTextAreaElement | null>(null);

    useEffect(() => {
        let alive = true;
        void api
            .persona()
            .then((fresh) => {
                if (!alive) return;
                setInfo(fresh);
                if (!edited) setDraft(fresh.persona);
            })
            .catch(() => {});
        return () => {
            alive = false;
        };
    }, []);

    const say = (id: string, note: CardNote) => setNotes((all) => ({ ...all, [id]: note }));
    const mark = (id: string, on: boolean) => setBusy((all) => ({ ...all, [id]: on }));

    /** Обновили и почистили редактор: он снова показывает то, что говорит сейчас. */
    const accepted = (fresh: PersonaInfo): void => {
        setInfo(fresh);
        setDraft(fresh.persona);
        setEdited(false);
        setBasis('');
    };

    /** Применить характер с карточки — Алиса говорит им со следующего вопроса. */
    const apply = async (preset: { id: string; title: string }) => {
        if (!isAdmin) return;
        setNotes({}); // подтверждение живёт только на последней нажатой карточке
        mark(preset.id, true);
        try {
            const fresh = await api.setPreset(preset.id);
            accepted(fresh);
            say(preset.id, { notice: preset.id === 'standard' ? 'Вернулись к стандартной.' : 'Говорит этим характером.' });
        } catch (e) {
            say(preset.id, { error: problemOf(e) });
        } finally {
            mark(preset.id, false);
        }
    };

    /** Взять характер в редактор: изменить и сохранить своим. */
    const toEditor = (title: string, text: string): void => {
        setDraft(text);
        setEdited(true);
        setBasis(title);
        editor.current?.focus();
    };

    /** Сохранить свой характер; пустой текст — вернуться к стандартному. */
    const save = async () => {
        if (!isAdmin) return;
        setNotes({}); // подтверждение живёт только на последней нажатой карточке
        mark('custom', true);
        try {
            const fresh = await api.setPersonaText(draft.trim());
            accepted(fresh);
            say('custom', { notice: draft.trim() ? 'Говорит вашим характером.' : 'Вернулись к стандартной.' });
        } catch (e) {
            say('custom', { error: problemOf(e) });
        } finally {
            mark('custom', false);
        }
    };

    if (!info) {
        return (
            <section class="card glass">
                <p class="t-meta m-0">Читаем характер…</p>
            </section>
        );
    }

    return (
        <section class="card glass card-list" aria-label="Манера">
            <div class="card-head">
                <h2 class="t-title-xs m-0">Манера</h2>
                <span class="t-meta">применяется со следующего вопроса</span>
            </div>
            {info.presets.map((preset) => {
                const active = info.activePreset === preset.id;
                return (
                    <Card
                        key={preset.id}
                        title={preset.title}
                        badge={preset.badge}
                        description={preset.description}
                        note={notes[preset.id]}
                        action={
                            active ? (
                                <span class="pill pill-own">говорит</span>
                            ) : isAdmin ? (
                                <div class="flex items-center gap-6">
                                    <Action label="применить" tone="on" busy={busy[preset.id]} onClick={() => void apply(preset)} />
                                    <Action label="изменить" busy={busy[preset.id]} onClick={() => toEditor(preset.title, preset.text)} />
                                </div>
                            ) : null
                        }
                    />
                );
            })}
            {isAdmin && (
                <Card
                    title="Свой характер"
                    description="Кем Алиса вам отвечает: кто она, как говорит, о себе — в женском или мужском роде."
                    note={notes['custom']}
                    action={info.activePreset === null ? <span class="pill pill-own">говорит</span> : null}
                >
                    <PersonaEditor
                        editorRef={editor}
                        draft={draft}
                        onDraft={(text) => {
                            setDraft(text);
                            setEdited(true);
                            setBasis('');
                        }}
                        basis={basis}
                        busy={busy['custom']}
                        onSave={() => void save()}
                    />
                </Card>
            )}
            {!isAdmin && <p class="t-caption mx-6 mt-8 mb-0">Менять характер может администратор.</p>}
        </section>
    );
}
