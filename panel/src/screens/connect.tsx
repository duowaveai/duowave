import { useEffect, useRef, useState } from 'preact/hooks';

import { ConnectJob, ConnectStepId, FoundStation, StationCheck, api, can, problemOf } from '../api';
import { IconCheck, IconMic, IconRobot, IconRoutes, IconVoice } from '../icons';
import { drawingOf } from '../stations';
import { confetti } from '../ui/confetti';
import { DuoWaveMark } from '../ui/duowave';
import { Sheet } from '../ui/sheet';

// «Подключить колонку»: мастер из плана открытия (docs/plans/open-source.md, раздел 6). Шлюз находит колонку,
// с согласия читает её настройки, показывает построчно, что изменит, и только по «Подключить» меняет: копия,
// два адреса, проверка, перезагрузка, «колонка пришла». Не пришла за 3 минуты — «Вернуть как было». Для тех, кто не
// пускает шлюз на колонку, — одна команда. Без root и SSH по вики шлюз колонку не подключает.

const WIKI_ROOT = 'https://hackalice.me/start/';
const WIKI_SSH = 'https://hackalice.me/start/ssh/';

const STEP_LABELS: Record<ConnectStepId, string> = {
    copy: 'Путь назад: старые адреса — в шлюзе, restore.sh — на колонке',
    write: 'Копия quasar.cfg с адресом шлюза — поверх файла прошивки',
    verify: 'Проверка: колонка читает копию',
    reboot: 'Перезагрузка колонки',
    arrive: 'Колонка выходит на связь со шлюзом',
};

const RESTORE_LABELS: Partial<Record<ConnectStepId, string>> = {
    write: 'Прежние адреса голосового сервера',
    verify: 'Проверка: колонка читает то, что задумано',
    reboot: 'Перезагрузка колонки',
};

/** Адрес шлюза, каким его видит колонка: адрес страницы, если он домашний; иначе его вводят. */
export function suggestedGateway(): string {
    const host = location.hostname;
    const home = /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host) || host.endsWith('.local') || !host.includes('.');
    return home && host !== 'localhost' ? location.host : '';
}

/** Что пошло не так — словами, с тем, что делать. */
function problemText(problem: string | undefined): string {
    switch (problem) {
        case 'no-ssh':
            return 'SSH на колонке не отвечает (порт 2222). Сначала root и SSH по вики hackalice — шаг 2.';
        case 'no-config':
            return 'На колонке нет quasar.cfg там, где его ждёт шлюз: эту модель он пока не знает.';
        case 'host-key-changed':
            return 'Ключ SSH у колонки не тот, что был: это другая колонка по тому же адресу или её перепрошили. Шлюз остановился, ничего не меняя.';
        case 'auth-failed':
            return 'Колонка не пустила по SSH: шлюз входит как root без пароля, как открывает вики.';
        case 'unreachable':
            return 'Колонка не отвечает по этому адресу: та же сеть? Не гостевая? Нет изоляции клиентов в роутере?';
        case 'no-arrival':
            return 'Колонка не пришла на шлюз за 3 минуты.';
        default:
            return problem ?? 'Что-то пошло не так.';
    }
}

function where(pointsTo: 'yandex' | 'gateway' | 'other' | undefined): string {
    return pointsTo === 'yandex' ? 'к Яндексу' : pointsTo === 'gateway' ? 'к шлюзу' : 'на незнакомый сервер';
}

function Steps({ job, labels }: { job: ConnectJob; labels: Partial<Record<ConnectStepId, string>> }) {
    return (
        <ol class="connect-steps">
            {job.steps.map((step) => (
                <li key={step.id} class={`connect-step is-${step.state}`}>
                    <span class="connect-mark" aria-hidden="true">
                        {step.state === 'done' ? <IconCheck /> : step.state === 'failed' ? '!' : step.state === 'skipped' ? '–' : ''}
                    </span>
                    <span>
                        {labels[step.id] ?? step.id}
                        <span class="sr-only">
                            {step.state === 'done' ? ' — готово' : step.state === 'running' ? ' — сейчас' : step.state === 'failed' ? ' — ошибка' : ''}
                        </span>
                    </span>
                </li>
            ))}
        </ol>
    );
}

/** Команда, которую нельзя ни потерять, ни опечатать: выделяется целиком, копируется кнопкой. */
function Command({ text }: { text: string }) {
    const [copied, setCopied] = useState(false);
    const field = useRef<HTMLTextAreaElement>(null);
    const copy = async () => {
        try {
            await navigator.clipboard.writeText(text);
            setCopied(true);
        } catch {
            // Буфер обмена — только на HTTPS: выделим, а скопирует человек.
            field.current?.select();
        }
    };
    return (
        <div class="connect-command">
            <textarea ref={field} class="field" readOnly rows={3} value={text} aria-label="Команда для колонки" onFocus={(e) => (e.target as HTMLTextAreaElement).select()} />
            <button class="pill pill-off" onClick={() => void copy()}>
                {copied ? 'Скопировано' : 'Скопировать'}
            </button>
        </div>
    );
}

type Step = 'find' | 'consent' | 'plan' | 'manual' | 'progress' | 'ready';

/** Порт, на который колонка пойдёт к шлюзу: из адреса «хост:порт», без него — порт самого шлюза (own). */
export function gatewayPort(address: string, own = 80): number {
    const port = /:(\d+)\s*$/.exec(address.trim())?.[1];
    return port ? Number(port) : own;
}

/**
 * Главный экран, пока колонок нет: что нужно, что сделает мастер и как вернуть как было — и кнопка, которая его
 * открывает. Без права управлять колонками — только что их подключает администратор.
 */
export function ConnectFirstCard({ onConnect }: { onConnect: () => void }) {
    const mayManage = can('stations.manage');
    return (
        <section class="card glass flex flex-col gap-12" aria-labelledby="connect-first-h">
            <h2 id="connect-first-h" class="t-title m-0">
                Подключите колонку
            </h2>
            <p class="t-meta m-0 c-text">
                DuoWave встаёт между колонкой и облаком: что ответить, решаете вы. Сначала колонку нужно направить на этот шлюз.
            </p>
            <ol class="t-meta m-0 connect-first-steps">
                <li>
                    Нужна Яндекс Станция с root и SSH — по{' '}
                    <a href={WIKI_ROOT} target="_blank" rel="noreferrer">
                        вики hackalice
                    </a>
                    . Шлюз root не делает.
                </li>
                <li>Мастер найдёт колонку в сети, прочитает её настройки и покажет, что изменит, — до вашего «Подключить».</li>
                <li>Сначала всё работает как раньше: отвечает Яндекс. Вернуть как было — одной кнопкой.</li>
            </ol>
            {mayManage ? (
                <button class="dash-done" onClick={onConnect}>
                    Подключить колонку
                </button>
            ) : (
                <p class="t-caption m-0">Колонку подключает администратор шлюза.</p>
            )}
        </section>
    );
}

/**
 * Мастер подключения: от поиска колонки до «пришла». onConnected — дальше имя и вид (SetupSheet); onOpenBrain — «Мозг»,
 * где добавляют свою модель (без неё отвечает Яндекс).
 */
export function ConnectSheet({
    onClose,
    onConnected,
    onOpenBrain,
    initial,
}: {
    onClose: () => void;
    onConnected: (device: string) => void;
    onOpenBrain?: () => void;
    /** Колонка и адрес шлюза заранее: «Переподключить», когда шлюз сменил порт. */
    initial?: { address: string; gateway: string };
}) {
    const [step, setStep] = useState<Step>('find');
    const [found, setFound] = useState<FoundStation[] | null>(null);
    const [address, setAddress] = useState(initial?.address ?? '');
    const [gateway, setGateway] = useState(initial?.gateway ?? suggestedGateway());
    /** Порт самого шлюза: на него колонка пойдёт, если адрес введён без порта. */
    const [ownPort, setOwnPort] = useState(80);
    const [check, setCheck] = useState<StationCheck | null>(null);
    const [job, setJob] = useState<ConnectJob | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Колонка вышла на связь — салют, один раз за подключение.
    const cheered = useRef<string | null>(null);
    useEffect(() => {
        if (job?.status !== 'done' || job.kind === 'restore' || cheered.current === job.id) return;
        cheered.current = job.id;
        confetti();
    }, [job?.id, job?.status]);

    /** ДиВи ведёт мастер: в шапке каждого шага он живой и оглядывается (ui/divi.ts, 'lively'). */
    const guide = <DuoWaveMark size={40} live idle="lively" />;

    useEffect(() => {
        let live = true;
        api.discoverStations()
            .then((result) => {
                if (!live) return;
                setFound(result.stations);
                if (result.port) setOwnPort(result.port);
                // Пульт открыт с этого компьютера (127.0.0.1): адрес для колонки подскажет сам шлюз.
                if (result.gateway?.length) setGateway((current) => current || result.gateway![0]);
            })
            .catch(() => live && setFound([]));
        return () => {
            live = false;
        };
    }, []);

    // Подключение идёт на шлюзе: пульт смотрит на него, пока оно не кончится.
    useEffect(() => {
        if (!job || job.status !== 'running') return;
        const timer = setInterval(() => {
            api.connectJob(job.id)
                .then(setJob)
                .catch(() => undefined);
        }, 1200);
        return () => clearInterval(timer);
    }, [job?.id, job?.status]);

    const run = async (work: () => Promise<void>) => {
        setBusy(true);
        setError(null);
        try {
            await work();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const read = () =>
        run(async () => {
            const result = await api.checkStation(address.trim(), gateway.trim() || undefined);
            setCheck(result);
            setStep('plan');
        });
    const connect = () =>
        run(async () => {
            setJob(await api.connectStation(check!.address, gateway.trim()));
            setStep('progress');
        });
    const restore = () =>
        run(async () => {
            setJob(await api.restoreStation(check!.address, 'previous'));
        });

    const problem = error && (
        <p role="alert" class="t-meta m-0 c-dot-offline">
            {error}
        </p>
    );

    if (step === 'find')
        return (
            <Sheet
                title="Подключить колонку"
                about="Колонка с root и SSH по вики hackalice"
                icon={guide}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" disabled={!address.trim()} onClick={() => setStep('consent')}>
                            Дальше
                        </button>
                        <button class="pill pill-off dash-wide" onClick={onClose}>
                            Отмена
                        </button>
                    </div>
                }
            >
                {found === null ? (
                    <p class="t-meta m-0">
                        Ищу колонки в сети…
                    </p>
                ) : found.length ? (
                    <div role="radiogroup" aria-label="Колонки в сети" class="flex flex-col">
                        {found.map((station) => (
                            <button
                                key={station.address}
                                role="radio"
                                aria-checked={address === station.address}
                                class="picker-row"
                                onClick={() => setAddress(station.address)}
                            >
                                <span class="dash-option-text">
                                    <b>{station.txt.platform ? drawingOf(null, station.txt.platform).title : station.name || 'Колонка'}</b>
                                    <span class="t-caption">
                                        {station.address} · {station.ssh ? 'SSH есть' : 'SSH нет — сначала шаг 2 вики'}
                                    </span>
                                </span>
                                <span class={`dash-radio${address === station.address ? ' on' : ''}`} aria-hidden="true">
                                    {address === station.address && <IconCheck />}
                                </span>
                            </button>
                        ))}
                    </div>
                ) : (
                    <p class="t-meta m-0">
                        В сети колонок не нашлось. Введите адрес — он есть в приложении Яндекса или в списке устройств роутера. Шлюз в
                        Docker колонки не видит: там адрес вводят всегда.
                    </p>
                )}
                <input
                    class="field"
                    value={address}
                    inputMode="decimal"
                    placeholder="Адрес колонки, например 192.168.1.23"
                    aria-label="Адрес колонки"
                    onInput={(event) => setAddress((event.target as HTMLInputElement).value)}
                    onKeyDown={(event) => {
                        if (event.key === 'Enter' && address.trim()) setStep('consent');
                    }}
                />
                <p class="t-caption m-0">
                    Колонка и шлюз — в одной сети, не гостевой, без изоляции клиентов в роутере. Нет root —{' '}
                    <a href={WIKI_ROOT} target="_blank" rel="noreferrer">
                        вики, шаг 1
                    </a>
                    ; нет SSH —{' '}
                    <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                        шаг 2
                    </a>
                    .
                </p>
            </Sheet>
        );

    if (step === 'consent')
        return (
            <Sheet
                title="Посмотреть колонку"
                about={address.trim()}
                icon={guide}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => void read()}>
                            {busy ? 'Читаю…' : 'Прочитать'}
                        </button>
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('find')}>
                            Назад
                        </button>
                    </div>
                }
            >
                <p class="t-meta m-0 c-text">
                    Шлюз зайдёт на колонку по SSH и прочитает: модель и прошивку, куда она сейчас отправляет голос, отключены ли
                    обновления, сколько места в /data. Ничего не изменит.
                </p>
                <label class="t-caption flex flex-col gap-6">
                    Адрес этого шлюза для колонки
                    <input
                        class="field"
                        value={gateway}
                        placeholder="Например, 192.168.1.50"
                        aria-label="Адрес шлюза для колонки"
                        onInput={(event) => setGateway((event.target as HTMLInputElement).value)}
                    />
                </label>
                <p class="t-caption m-0">
                    Колонка пойдёт на порт {gatewayPort(gateway, ownPort)} — он должен быть открыт. Закрепите адрес компьютера в роутере
                    (резервирование DHCP), иначе однажды колонка его потеряет.
                </p>
                {problem}
            </Sheet>
        );

    if (step === 'plan' && check) {
        const facts = check.facts;
        const ready = !check.problem && !!facts && !!check.target;
        return (
            <Sheet
                title={check.problem ? 'Подключить не выйдет' : check.connected ? 'Уже подключена' : 'Что изменится'}
                icon={guide}
                about={
                    facts
                        ? [facts.deviceType && drawingOf(null, facts.deviceType).title, facts.softwareVersion && `прошивка ${facts.softwareVersion}`, check.address]
                              .filter(Boolean)
                              .join(' · ')
                        : check.address
                }
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        {ready && !check.connected && (
                            <button class="dash-done" disabled={busy || !gateway.trim()} onClick={() => void connect()}>
                                {busy ? 'Подключаю…' : 'Подключить'}
                            </button>
                        )}
                        {ready && !check.connected && (
                            <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('manual')}>
                                Сделаю сам
                            </button>
                        )}
                        {(!ready || check.connected) && (
                            <button class="dash-done" onClick={onClose}>
                                Закрыть
                            </button>
                        )}
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('consent')}>
                            Назад
                        </button>
                    </div>
                }
            >
                {check.problem ? (
                    <p class="t-meta m-0 c-text">
                        {problemText(check.problem)}{' '}
                        {(check.problem === 'no-ssh' || check.problem === 'auth-failed') && (
                            <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                                SSH по вики
                            </a>
                        )}
                    </p>
                ) : check.connected ? (
                    <p class="t-meta m-0 c-text">
                        Колонка уже отправляет голос на этот шлюз. Если её нет в списке — она подождёт одобрения там.
                    </p>
                ) : (
                    facts &&
                    check.target && (
                        <>
                            <ul class="connect-plan">
                                <li>
                                    Голосовой сервер: <code>{facts.addresses.url}</code> → <code>{check.target.url}</code>
                                </li>
                                <li>
                                    Адрес QUIC: <code>{facts.addresses.quic || 'пусто'}</code> → пусто, чтобы голос не уходил к Яндексу мимо шлюза
                                </li>
                                <li>
                                    Файл прошивки <code>quasar.cfg</code> не меняется: поверх него подключается наша копия из{' '}
                                    <code>/data/opt/duowave</code>, при каждой загрузке заново. Сброс колонки или обновление прошивки — и это
                                    снова обычная Алиса. Откат без шлюза — <code>/data/opt/duowave/restore.sh</code>.
                                </li>
                                <li>Колонка перезагрузится — около минуты — и придёт на шлюз. Не придёт за 3 минуты — шлюз предложит вернуть как было.</li>
                            </ul>
                            {facts.pointsTo === 'gateway' ? (
                                <p class="t-meta connect-warning m-0">
                                    Колонка уже работает с другим шлюзом: <code>{facts.addresses.url}</code>. После переключения он её потеряет;
                                    «Вернуть как было» вернёт её туда.
                                </p>
                            ) : (
                                <p class="t-caption m-0">
                                    Сейчас голос идёт {where(facts.pointsTo)}. После подключения колонка работает как раньше — режим «Яндекс», а
                                    своё включают по одному в «Мозг» → «Маршруты».
                                </p>
                            )}
                            {check.updates === 'on' && (
                                <p class="t-meta connect-warning m-0">
                                    Обновления от Яндекса не отключены. Обновление вернёт колонку к Яндексу и может закрыть root —{' '}
                                    <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                                        отключите по вики
                                    </a>
                                    .
                                </p>
                            )}
                            {check.copy?.refused === 'firmware-changed' && (
                                <p class="t-meta connect-warning m-0">
                                    Колонка обновилась: прошивка сменила свой quasar.cfg, и наша копия при загрузке не
                                    подключилась — сейчас это обычная Алиса. «Подключить» соберёт копию заново.
                                </p>
                            )}
                            {check.copy?.refused === 'gateway-off' && (
                                <p class="t-meta m-0">
                                    Шлюз на самой колонке выключен страховкой: голос колонки пока идёт мимо него.
                                </p>
                            )}
                            {check.dataFreeKb !== undefined && check.dataFreeKb < 2048 && (
                                <p class="t-meta m-0">
                                    В /data меньше 2 МБ: копии конфига может не хватить места.
                                </p>
                            )}
                        </>
                    )
                )}
                {problem}
            </Sheet>
        );
    }

    if (step === 'manual' && check) {
        const command = `ssh -p 2222 root@${check.address} 'wget -qO- http://${gateway.trim()}/station/connect.sh | sh'`;
        return (
            <Sheet
                title="Сделаю сам"
                about="Одна команда на компьютере с ssh: macOS, Linux, Windows 10 и новее"
                icon={guide}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" onClick={onClose}>
                            Готово
                        </button>
                        <button class="pill pill-off dash-wide" onClick={() => setStep('plan')}>
                            Назад
                        </button>
                    </div>
                }
            >
                <Command text={command} />
                <p class="t-caption m-0">
                    Колонка возьмёт скрипт у шлюза и сделает то же, что кнопка: копия, два адреса, проверка, перезагрузка. Откат без
                    шлюза: <code>ssh -p 2222 root@{check.address} sh /data/opt/duowave/restore.sh</code>. После перезагрузки колонка
                    появится в списке и подождёт, пока вы её одобрите.
                </p>
            </Sheet>
        );
    }

    if (step === 'ready' && job?.device)
        return (
            <Sheet
                title="Колонка подключена"
                about="Голос идёт через шлюз, отвечает пока Яндекс"
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" onClick={() => onConnected(job.device!)}>
                            Дальше: имя и вид
                        </button>
                        {onOpenBrain && (
                            <button class="pill pill-off dash-wide" onClick={onOpenBrain}>
                                Своя модель — в «Мозге»
                            </button>
                        )}
                    </div>
                }
            >
                {/* ДиВи радуется вместе с салютом (эффект выше), потом снова оглядывается. */}
                <div class="connect-hero">
                    <DuoWaveMark size={72} live cheer idle="lively" />
                </div>
                <p class="connect-now m-0">
                    <span class="pill pill-yandex">Яндекс</span>
                    <span class="t-meta">Сейчас отвечает Яндекс, как раньше: колонка работает как обычно, только уже через шлюз. Своё включается по одному.</span>
                </p>
                <ol class="connect-next">
                    <li>
                        <span class="connect-next-icon" aria-hidden="true">
                            <IconMic size={16} />
                        </span>
                        <div class="connect-next-body">
                            <b>Проверьте голосом</b>
                            <span class="connect-say">«Алиса, сколько времени?»</span>
                            <span class="t-caption">Вопрос и ответ появятся на главном экране пульта.</span>
                        </div>
                    </li>
                    <li>
                        <span class="connect-next-icon" aria-hidden="true">
                            <IconRobot size={16} />
                        </span>
                        <div class="connect-next-body">
                            <b>Своя модель</b>
                            <span class="t-caption">
                                <span class="connect-path">Мозг → Модели</span> ключ Claude или OpenAI — или Ollama у вас дома.
                            </span>
                            <span class="connect-say">«Алиса, придумай сказку про кота»</span>
                            <span class="t-caption">— ответит уже ваша модель.</span>
                        </div>
                    </li>
                    <li>
                        <span class="connect-next-icon" aria-hidden="true">
                            <IconRoutes size={16} />
                        </span>
                        <div class="connect-next-body">
                            <b>Кто что отвечает</b>
                            <span class="t-caption">
                                <span class="connect-path">Мозг → Маршруты</span> например: погода и музыка — Яндекс, разговор — своя модель.
                            </span>
                        </div>
                    </li>
                    <li>
                        <span class="connect-next-icon" aria-hidden="true">
                            <IconVoice size={16} />
                        </span>
                        <div class="connect-next-body">
                            <b>Свой голос и слух</b>
                            <span class="t-caption">
                                <span class="connect-path">Голос</span> голос по записи или готовые, своё распознавание. У каждого есть «Проверить».
                            </span>
                        </div>
                    </li>
                </ol>
            </Sheet>
        );

    // progress: подключение, а при «не пришла» — откат на том же экране.
    const restoring = job?.kind === 'restore';
    return (
        <Sheet
            title={restoring ? 'Возвращаю как было' : 'Подключаю колонку'}
            about={check?.address}
            onClose={onClose}
            dismissable={job?.status !== 'running'}
            footer={
                job?.status === 'done' && !restoring ? (
                    <button class="dash-done" onClick={() => setStep('ready')}>
                        Дальше
                    </button>
                ) : job?.status === 'timeout' ? (
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => void restore()}>
                            Вернуть как было
                        </button>
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={onClose}>
                            Оставить
                        </button>
                    </div>
                ) : job?.status === 'running' ? (
                    <button class="dash-done" disabled>
                        Подождите…
                    </button>
                ) : (
                    <button class="dash-done" onClick={onClose}>
                        Закрыть
                    </button>
                )
            }
        >
            {!restoring && (
                <div class="connect-hero">
                    <DuoWaveMark size={72} live idle="lively" />
                </div>
            )}
            {job && <Steps job={job} labels={restoring ? RESTORE_LABELS : STEP_LABELS} />}
            {job?.status === 'timeout' && (
                <p class="t-meta m-0 c-text">
                    {problemText('no-arrival')} Проверьте адрес шлюза и что порт {gatewayPort(gateway, ownPort)} открыт. «Вернуть как было» поставит прежние адреса и
                    перезагрузит колонку.
                </p>
            )}
            {job?.status === 'failed' && (
                <p role="alert" class="t-meta m-0 c-text">
                    {problemText(job.error)}
                    {job.copy ? ` Наша копия на колонке: ${job.copy}.` : ''}
                </p>
            )}
            {job?.status === 'done' && restoring && (
                <p class="t-meta m-0 c-text">
                    Прежние адреса на месте, колонка перезагружается и вернётся к Яндексу.
                </p>
            )}
            {problem}
        </Sheet>
    );
}

/** «Вернуть к Яндексу»: прежние адреса голосового сервера и перезагрузка — по подтверждению. */
export function RestoreSheet({ address, name, onClose }: { address: string; name: string; onClose: () => void }) {
    const [job, setJob] = useState<ConnectJob | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (!job || job.status !== 'running') return;
        const timer = setInterval(() => {
            api.connectJob(job.id)
                .then(setJob)
                .catch(() => undefined);
        }, 1200);
        return () => clearInterval(timer);
    }, [job?.id, job?.status]);

    const start = async () => {
        setBusy(true);
        setError(null);
        try {
            setJob(await api.restoreStation(address));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <Sheet
            title="Вернуть к Яндексу"
            about={`${name} · ${address}`}
            onClose={onClose}
            dismissable={job?.status !== 'running'}
            footer={
                !job ? (
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => void start()}>
                            {busy ? 'Начинаю…' : 'Вернуть'}
                        </button>
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={onClose}>
                            Отмена
                        </button>
                    </div>
                ) : (
                    <button class="dash-done" disabled={job.status === 'running'} onClick={onClose}>
                        {job.status === 'running' ? 'Подождите…' : 'Закрыть'}
                    </button>
                )
            }
        >
            {!job ? (
                <p class="t-meta m-0 c-text">
                    Шлюз зайдёт на колонку по SSH, поставит голосовому серверу прежние адреса — те, что были до шлюза, — и перезагрузит её.
                    Колонка будет работать с Яндексом напрямую, без шлюза.
                </p>
            ) : (
                <Steps job={job} labels={RESTORE_LABELS} />
            )}
            {job?.status === 'failed' && (
                <p role="alert" class="t-meta m-0 c-text">
                    {problemText(job.error)} Без шлюза: <code>ssh -p 2222 root@{address} sh /data/opt/duowave/restore.sh</code>
                </p>
            )}
            {job?.status === 'done' && (
                <p class="t-meta m-0 c-text">
                    Готово: колонка перезагружается и вернётся к Яндексу.
                </p>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
