import { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { PanelUser, api, problemOf, sessionStore } from '../api';
import { DuoWaveMark } from '../ui/duowave';

/**
 * Первый запуск и вход. Визард задаёт пароль администратора — первый вошедший им и становится; дальше вход
 * паролем (имя появится, когда администратор добавит людей). Это же место потом обрастает шагами: люди,
 * обратные звонки, агенты, обновления.
 */
export function AuthScreen({
    mode,
    accounts,
    needsCode = false,
    onDone,
}: {
    mode: 'setup' | 'login';
    /** Сколько людей уже есть: со второго поле имени обязательно. */
    accounts: number;
    /** Первая настройка не с этого компьютера: нужен код из журнала шлюза. */
    needsCode?: boolean;
    onDone: () => void;
}) {
    const [name, setName] = useState('');
    const [password, setPassword] = useState('');
    const [repeat, setRepeat] = useState('');
    const [code, setCode] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const finish = (session: { token: string; user: PanelUser }): void => {
        sessionStore.set({ token: session.token, name: session.user.name, role: session.user.role });
        onDone();
    };

    const submit = async () => {
        setError(null);
        if (mode === 'setup' && password.length < 8) return setError('Пароль — хотя бы 8 символов.');
        if (mode === 'setup' && password !== repeat) return setError('Пароли не совпадают.');
        if (mode === 'setup' && needsCode && !code.trim()) return setError('Введите код настройки из журнала шлюза.');
        if (mode === 'login' && accounts > 1 && name.trim().length < 2) return setError('Введите имя.');
        setBusy(true);
        try {
            const session =
                mode === 'setup' ? await api.setup(password, needsCode ? code.trim() : undefined) : await api.login(name, password);
            finish(session);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const onEnter = (e: KeyboardEvent) => e.key === 'Enter' && void submit();

    /** Поле входа: капсула на стекле. */
    const field = 'py-12 px-16 r-pill border-glass bg-icon-bg';

    return (
        <div class="screen justify-center min-h-screen">
            <section class="card glass flex flex-col gap-14 p-24">
                {/* ДиВи встречает: оглядывается, моргает, изредка здоровается (ui/divi.ts, 'lively'). */}
                <div class="auth-hero">
                    <DuoWaveMark size={96} live idle="lively" />
                </div>
                <div class="auth-head">
                    <h1 class="t-title m-0">{mode === 'setup' ? 'Добро пожаловать в DuoWave' : 'Пульт'}</h1>
                    <p class="t-meta m-0 mt-4">
                        {mode === 'setup'
                            ? 'Шаг 1 из 2 — пароль администратора: первый, кто его задаёт, управляет пультом. Шаг 2 — подключим колонку.'
                            : accounts > 1
                              ? 'Имя и пароль — как администратор вас записал.'
                              : 'Введите пароль администратора.'}
                    </p>
                </div>

                {mode === 'setup' && needsCode && (
                    <Field
                        label="Код настройки"
                        hint="Строка «первая настройка пульта» в журнале шлюза: на Mac — logs/duowave.log, в Docker — docker compose logs gateway."
                    >
                        <input
                            value={code}
                            onInput={(e) => setCode((e.target as HTMLInputElement).value)}
                            onKeyDown={onEnter}
                            placeholder="1234-5678"
                            aria-label="Код настройки"
                            inputMode="numeric"
                            autoComplete="one-time-code"
                            autoFocus
                            class={field}
                        />
                    </Field>
                )}
                {mode === 'login' && accounts > 1 && (
                    <Field label="Имя">
                        <input
                            value={name}
                            onInput={(e) => setName((e.target as HTMLInputElement).value)}
                            onKeyDown={onEnter}
                            autoComplete="username"
                            aria-label="Имя"
                            autoFocus
                            class={field}
                        />
                    </Field>
                )}
                <Field label={mode === 'setup' ? 'Пароль администратора' : 'Пароль'}>
                    <input
                        type="password"
                        value={password}
                        onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
                        onKeyDown={onEnter}
                        placeholder={mode === 'setup' ? 'от 8 символов' : undefined}
                        aria-label={mode === 'setup' ? 'Пароль администратора' : 'Пароль'}
                        autoComplete={mode === 'setup' ? 'new-password' : 'current-password'}
                        autoFocus={!(mode === 'setup' && needsCode) && !(mode === 'login' && accounts > 1)}
                        class={field}
                    />
                </Field>
                {mode === 'setup' && (
                    <Field label="Пароль ещё раз">
                        <input
                            type="password"
                            value={repeat}
                            onInput={(e) => setRepeat((e.target as HTMLInputElement).value)}
                            onKeyDown={onEnter}
                            autoComplete="new-password"
                            aria-label="Пароль ещё раз"
                            class={field}
                        />
                    </Field>
                )}

                {error && (
                    <p class="m-0 fs-13 lh-18 c-dot-offline" role="alert">
                        {error}
                    </p>
                )}

                <button
                    class="pill justify-center min-h-48 bg-accent c-on-accent fs-15 shadow-talk"
                    disabled={busy}
                    onClick={() => void submit()}
                >
                    {busy ? 'Секунду…' : mode === 'setup' ? 'Сохранить и войти' : 'Войти'}
                </button>

                <p class="t-caption m-0">
                    {mode === 'setup'
                        ? needsCode
                            ? 'Вы настраиваете не с компьютера шлюза, поэтому нужен код. Пароль хранится в базе шлюза в виде хэша.'
                            : 'Пароль хранится в базе шлюза в виде хэша. Людей и их права добавите потом: «Настройки» → «Люди и доступ».'
                        : 'Пульт запомнит это устройство на месяц.'}
                </p>
            </section>
        </div>
    );
}

/** Поле с подписью сверху: и заполненное, оно говорит, что в нём, — код не спутать с паролем. Подсказка — под полем,
 * вне подписи: она описывает поле, а не называет его. */
function Field({ label, hint, children }: { label: string; hint?: string; children: ComponentChildren }) {
    return (
        <div class="flex flex-col gap-6">
            <label class="flex flex-col gap-6">
                <span class="t-caption fw-700 c-text">{label}</span>
                {children}
            </label>
            {hint && <span class="t-caption my-0 mx-4">{hint}</span>}
        </div>
    );
}
