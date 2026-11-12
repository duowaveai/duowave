import { useEffect, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';

import { ApiError, SignInStart, api, problemOf } from '../api';
import { Action } from './cards';

/** Как часто пульт спрашивает шлюз, вошёл ли человек. */
const POLL_MS = 2000;

export interface SignInFlow {
    /** Кто входит сейчас (id провайдера); null — никто. */
    target: string | null;
    /** Вход начинается: шлюз ещё не дал страницу. */
    starting: boolean;
    /** Начать вход. Вызывать прямо из нажатия: окно для страницы сервиса открывается сразу, иначе браузер его закроет. */
    start: (target: string, service: string, begin: () => Promise<SignInStart>) => void;
    /** Шаги входа под кнопкой: ссылка на страницу, поле для кода, ошибка. */
    steps: JSX.Element | null;
}

/**
 * Вход в аккаунт сервиса вместо ключа — «Войти через OpenRouter», «Continue with ChatGPT». Страница сервиса
 * открывается в новой вкладке, а в приложении DuoWave — в браузере, по ссылке. Пульт открыт на компьютере шлюза —
 * браузер после входа вернётся в шлюз сам. Пульт на другом устройстве — страница сервиса покажет код (OpenRouter) или
 * браузер — ошибку адреса 127.0.0.1 (ChatGPT): код или этот адрес вставляют сюда. Пока вход не закончен, пульт раз в
 * две секунды спрашивает шлюз, как дела.
 */
export function useSignIn(onDone: (target: string, title: string) => void): SignInFlow {
    const [target, setTarget] = useState<string | null>(null);
    const [service, setService] = useState('');
    const [started, setStarted] = useState<SignInStart | null>(null);
    const [opened, setOpened] = useState(false);
    const [starting, setStarting] = useState(false);
    const [finishing, setFinishing] = useState(false);
    const [text, setText] = useState('');
    const [error, setError] = useState<string | null>(null);
    const done = useRef(onDone);
    done.current = onDone;

    const close = () => {
        setTarget(null);
        setStarted(null);
        setOpened(false);
        setText('');
        setError(null);
    };

    const succeed = (who: string, title: string) => {
        close();
        done.current(who, title);
    };

    useEffect(() => {
        if (!started || !target) return;
        let live = true;
        const timer = setInterval(async () => {
            try {
                const status = await api.signInStatus(started.id);
                if (!live) return;
                if (status.state === 'done') succeed(target, status.title);
                else if (status.state === 'failed') {
                    setStarted(null);
                    setError(status.message);
                }
            } catch (e) {
                // Шлюз этот вход уже не помнит (устарел, шлюз перезапустился); сбой связи — спросим ещё раз.
                if (live && e instanceof ApiError && e.status === 404) {
                    setStarted(null);
                    setError(problemOf(e));
                }
            }
        }, POLL_MS);
        return () => {
            live = false;
            clearInterval(timer);
        };
    }, [started?.id]);

    const start = (who: string, name: string, begin: () => Promise<SignInStart>) => {
        setTarget(who);
        setService(name);
        setStarted(null);
        setOpened(false);
        setText('');
        setError(null);
        setStarting(true);
        // Окно открывается сейчас, по нажатию: открытое после ответа шлюза браузер сочтёт всплывающим. В приложении
        // окна нет — страницу откроет ссылка ниже, в браузере.
        const page = window.open('about:blank', '_blank');
        if (page) page.opener = null;
        begin()
            .then((fresh) => {
                setStarted(fresh);
                try {
                    if (page && !page.closed) {
                        page.location.href = fresh.url;
                        setOpened(true);
                    }
                } catch {
                    // Окно закрыли, пока шлюз отвечал: страницу откроет ссылка.
                }
            })
            .catch((e) => {
                page?.close();
                setError(problemOf(e));
            })
            .finally(() => setStarting(false));
    };

    const finish = async () => {
        if (!started || !target) return;
        setFinishing(true);
        setError(null);
        try {
            const result = await api.completeSignIn(started.id, text.trim());
            succeed(target, result.title);
        } catch (e) {
            setError(problemOf(e));
            // Сервис не принял код — этот вход закончен, нужен новый; пустое поле или опечатка в адресе — можно ещё раз.
            const after = await api.signInStatus(started.id).catch(() => null);
            if (after?.state !== 'waiting') setStarted(null);
        } finally {
            setFinishing(false);
        }
    };

    if (!target || (!started && !error)) return { target, starting, start, steps: null };

    const mode = started?.mode;
    const code = mode === 'code';
    const say = {
        here: `Войдите на странице ${service} — шлюз получит доступ сам, пульт это увидит.`,
        code: `Войдите на странице ${service}: она покажет код — скопируйте его и вставьте сюда.`,
        address: `Войдите на странице ${service}. После входа браузер покажет ошибку адреса 127.0.0.1 — так и должно быть: скопируйте адрес той страницы и вставьте сюда.`,
    };
    const placeholder = {
        here: 'Не вернулось само? Адрес страницы после входа',
        code: `Код со страницы ${service}`,
        address: 'http://127.0.0.1:…/auth/callback?code=…',
    };
    const steps = (
        <div class="flex flex-col gap-6">
            {started && mode && (
                <>
                    <span class="t-caption">{say[mode]}</span>
                    <a class="t-caption c-accent-text self-start" href={started.url} target="_blank" rel="noopener noreferrer">
                        {opened ? `Открыть страницу ${service} ещё раз` : `Открыть страницу ${service}`}
                    </a>
                    <form
                        class="flex gap-8 flex-wrap"
                        onSubmit={(e) => {
                            e.preventDefault();
                            void finish();
                        }}
                    >
                        <input
                            class="field flex-1-1-220 min-w-0"
                            autocomplete="off"
                            aria-label={code ? `Код со страницы ${service}` : 'Адрес страницы после входа'}
                            placeholder={placeholder[mode]}
                            value={text}
                            onInput={(e) => setText((e.target as HTMLInputElement).value)}
                        />
                        <Action label="завершить вход" tone="on" disabled={!text.trim()} busy={finishing} onClick={() => void finish()} />
                        <Action label="отменить" disabled={finishing} onClick={close} />
                    </form>
                </>
            )}
            {error && (
                <div class="flex gap-8 flex-wrap items-center">
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {error}
                    </p>
                    {!started && <Action label="закрыть" onClick={close} />}
                </div>
            )}
        </div>
    );
    return { target, starting, start, steps };
}
