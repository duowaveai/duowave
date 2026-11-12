import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, AsrCard, DownloadState, HomeVoice, ModelsView, TtsCard, TtsPending, VoiceNeeds, api, can, isTight, needsOf, problemOf } from '../api';
import { IconTimer, IconVoice } from '../icons';
import { Action, Card, CardNote } from '../ui/cards';
import { Dropdown } from '../ui/dropdown';
import { Progress, needsWhat, useNeedsFlow, voicePill } from '../ui/needs';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { OwnVoiceRows, OwnVoicesHeader, VoiceWizard } from './voice-lab';
import { localAsrLine, localAsrPercent } from './local-asr';
import { ask } from '../ui/confirm';

export type ModelsTab = 'voice' | 'recognition';
type Tab = ModelsTab;

/** Меньше этого от низа страницы — догружаем карточки. */
const NEAR_BOTTOM_PX = 600;

/** Языки реестра голосов: код из локали → имя на пилюле. Нет в списке — покажем сам код. */
const LANGUAGES: Record<string, string> = {
    ru: 'Русский',
    en: 'English',
    uk: 'Українська',
    de: 'Deutsch',
    es: 'Español',
    fr: 'Français',
    it: 'Italiano',
    pl: 'Polski',
    pt: 'Português',
    cs: 'Čeština',
    sk: 'Slovenčina',
    nl: 'Nederlands',
    sv: 'Svenska',
    da: 'Dansk',
    fi: 'Suomi',
    nb: 'Norsk',
    no: 'Norsk',
    ro: 'Română',
    hu: 'Magyar',
    el: 'Ελληνικά',
    tr: 'Türkçe',
    ar: 'العربية',
    fa: 'فارسی',
    ka: 'ქართული',
    kk: 'Қазақша',
    bn: 'বাংলা',
    gu: 'ગુજરાતી',
    ha: 'Hausa',
    ne: 'नेपाली',
    sw: 'Kiswahili',
    vi: 'Tiếng Việt',
    zh: '中文',
    yue: '粤语',
    ja: '日本語',
    sr: 'Српски',
    is: 'Íslenska',
    lb: 'Lëtzebuergesch',
    eo: 'Esperanto',
};

/** Качество пайпера: код реестра → подпись на пилюле, от лёгкого к тяжёлому. */
const QUALITIES: Array<{ id: string; label: string }> = [
    { id: 'x_low', label: 'совсем лёгкое' },
    { id: 'low', label: 'лёгкое' },
    { id: 'medium', label: 'среднее' },
    { id: 'high', label: 'высокое' },
];

const langOf = (locale?: string): string => (locale ? locale.split('_')[0] : '');

/** Прогресс скачивания голоса — заметная линия на всю ширину карточки. */
function DownloadBar({ download }: { download: DownloadState }) {
    if (download.state === 'error')
        return (
            <span class="t-caption c-dot-offline" role="alert">
                {download.error}
            </span>
        );
    const percent = Math.round((download.bytes / Math.max(download.total, 1)) * 100);
    if (download.state === 'done' || download.state === 'canceled') return null;
    return (
        <div class="flex items-center gap-10">
            <div role="progressbar" aria-label="Скачивание" aria-valuenow={percent} class="flex-1 h-8 r-4 bg-track overflow-hidden">
                <div class="progress-fill" style={{ width: `${percent}%`, transition: 'width .3s' }} />
            </div>
            <span class="t-caption fw-700 flex-none">
                {percent}%
            </span>
        </div>
    );
}

/**
 * Голос, который ждёт свою модель или файл («Скачать и включить»): ставится, памяти впритык — «всё равно?», не
 * вышло — ещё раз. «Отменить» забывает отложенный выбор (DELETE /api/v1/models/tts/pending).
 */
function PendingVoice({ pending, title, isAdmin, onChanged }: { pending: TtsPending; title: string; isAdmin: boolean; onChanged: () => void }) {
    const [busy, setBusy] = useState<'cancel' | 'anyway' | 'retry' | null>(null);
    const [error, setError] = useState<string | null>(null);
    const run = async (what: 'cancel' | 'anyway' | 'retry', action: () => Promise<unknown>) => {
        setBusy(what);
        setError(null);
        try {
            await action();
            onChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const voiceId = pending.voiceId ?? undefined;
    // Чего ждём: шлюз говорит это в waitFor; needs — запасной путь для старого ответа.
    const kind = pending.waitFor?.kind ?? pending.needs?.kind ?? 'engine';
    const what = pending.needs ? needsWhat(pending.needs) : kind === 'file' ? 'голос' : kind === 'imprint' ? 'снимок голоса' : 'модель';
    return (
        <section class="card glass card-list" aria-labelledby="pending-h">
            <div class="card-head">
                <h2 id="pending-h" class="t-title-xs m-0">
                    Голос ждёт
                </h2>
                <span class="t-meta">«{title}»</span>
            </div>
            <div class="pending-row">
                {pending.state === 'tight' ? (
                    <span class="t-body-sm c-pill-yandex-fg" role="status">
                        Всё встало, но памяти впритык{pending.reason ? `: ${pending.reason}` : '.'} Включить «{title}» всё равно?
                    </span>
                ) : pending.state === 'failed' ? (
                    <span class="t-body-sm c-dot-offline" role="alert">
                        Не вышло {kind === 'file' ? 'скачать' : kind === 'imprint' ? 'снять' : 'поставить'} {what}
                        {pending.reason ? `: ${pending.reason}` : '.'}
                    </span>
                ) : (
                    <>
                        <span class="t-body-sm">
                            {kind === 'file'
                                ? `Скачиваю ${what} — «${title}» включится сам, когда файл скачается.`
                                : `Ставлю ${what} — «${title}» включится сам, когда она встанет.`}
                        </span>
                        {pending.progress !== undefined && <Progress percent={pending.progress} label={`Установка: ${title}`} />}
                    </>
                )}
                {error && (
                    <span class="t-caption c-dot-offline" role="alert">
                        {error}
                    </span>
                )}
                {isAdmin && (
                    <div class="flex flex-wrap items-center gap-8">
                        {pending.state === 'tight' && (
                            <Action
                                tone="on"
                                label="Включить всё равно"
                                busy={busy === 'anyway'}
                                disabled={!!busy}
                                onClick={() => void run('anyway', () => api.activateTts(pending.modelId, voiceId, true))}
                            />
                        )}
                        {pending.state === 'failed' && (
                            <Action
                                tone="on"
                                label="Попробовать снова"
                                busy={busy === 'retry'}
                                disabled={!!busy}
                                onClick={() => void run('retry', () => api.activateTts(pending.modelId, voiceId, false, true))}
                            />
                        )}
                        <Action label="Отменить" busy={busy === 'cancel'} disabled={!!busy} onClick={() => void run('cancel', () => api.cancelTtsPending())} />
                    </div>
                )}
            </div>
        </section>
    );
}

/** Голос и распознавание — вкладки сверху; мозг и характер живут в разделе «Мозг». */
const MODELS_TABS: Array<TabEntry<ModelsTab>> = [
    { id: 'voice', label: 'Голос', icon: <IconVoice size={16} /> },
    { id: 'recognition', label: 'Слух', icon: <IconTimer size={16} /> },
];

/**
 * «Голос» колонки, пока компьютер даёт ей свой: голос компьютера (наши ответы — им, ответы Яндекса — голосом Алисы)
 * или Алиса. Выбор — самой колонки; перенос с компьютером-помощником приносит выбор компьютера. Шлюз постарше
 * маршрута не знает — тогда карточки нет.
 */
function HomeVoiceChoice() {
    const [state, setState] = useState<HomeVoice | null>(null);
    const [busy, setBusy] = useState<HomeVoice['chosen'] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const isAdmin = can('models.manage');

    useEffect(() => {
        let alive = true;
        const read = () =>
            api
                .homeVoice()
                .then((fresh) => alive && setState(fresh))
                .catch(() => {});
        void read();
        const id = setInterval(read, 30_000);
        return () => {
            alive = false;
            clearInterval(id);
        };
    }, []);

    if (!state?.offered) return null;
    const pick = async (voice: HomeVoice['chosen']) => {
        if (busy || voice === state.chosen) return;
        setBusy(voice);
        setError(null);
        try {
            setState(await api.setHomeVoice(voice));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const computer = state.voice ? `Голос компьютера (${state.voice})` : 'Голос компьютера';
    return (
        <section class="card glass py-12 px-16 mb-10" aria-label="Чей голос отвечает">
            <h2 class="t-title-xs m-0 mb-6">Чей голос отвечает</h2>
            <p class="t-body m-0 mb-10">
                {state.chosen === 'computer'
                    ? !state.up
                        ? 'Компьютер не на связи — пока он не вернётся, отвечает голос Алисы.'
                        : state.provider === 'own'
                          ? 'Все ответы — голосом компьютера.'
                          : 'Наши ответы — голосом компьютера, ответы Яндекса — голосом Алисы.'
                    : 'Отвечает голос Алисы.'}
            </p>
            <div class="flex flex-wrap gap-8">
                <Action
                    on={state.chosen === 'computer'}
                    label={computer}
                    busy={busy === 'computer'}
                    disabled={!isAdmin || (!!busy && busy !== 'computer')}
                    onClick={() => void pick('computer')}
                />
                <Action
                    tone={state.chosen === 'alice' ? 'yandex' : 'off'}
                    label="Алиса"
                    busy={busy === 'alice'}
                    disabled={!isAdmin || (!!busy && busy !== 'alice')}
                    onClick={() => void pick('alice')}
                />
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0 mt-8" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}

/**
 * Шлюз на самой колонке: своих моделей и голосовой лаборатории в нём нет. Голос и распознавание задаются адресами
 * облачных сервисов в .env шлюза; мозг и характер — как обычно, в разделе «Мозг». Пока компьютер даёт колонке свой
 * голос, во вкладке «Голос» — выбор между ним и Алисой (HomeVoiceChoice).
 */
function LocalModelsAbsent({ kind, message }: { kind: Tab; message: string }) {
    return (
        <>
            {kind === 'voice' && <HomeVoiceChoice />}
            <section class="card glass py-12 px-16" aria-label={kind === 'voice' ? 'Голос' : 'Слух'}>
                <h2 class="t-title-xs m-0 mb-6">
                    {kind === 'voice' ? 'Свои голоса — на компьютере' : 'Своё распознавание — на компьютере'}
                </h2>
                <p class="t-body m-0 mb-6">
                    {message}
                </p>
                <p class="t-caption m-0">
                    {kind === 'voice'
                        ? 'Без голоса компьютера здесь отвечает голос Алисы. Свой голос — облачный синтез с OpenAI-совместимым /audio/speech: TTS_PROVIDER и TTS_OWN_URL в .env шлюза.'
                        : 'Своё распознавание здесь — облачное, с OpenAI-совместимым /audio/transcriptions: ASR_BASE_URL и ASR_API_KEY в .env шлюза.'}
                </p>
            </section>
        </>
    );
}

/** `bare` — без своих вкладок: голос стоит в «Персонаже», слух — в «Настройки» → «Колонка» (docs/plans/panel-rebuild.md). */
export function ModelsScreen({ tab, onTab, bare = false }: { tab: ModelsTab; onTab: (tab: ModelsTab) => void; bare?: boolean }) {
    const [view, setView] = useState<ModelsView | null>(null);
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const [busy, setBusy] = useState<Record<string, boolean>>({});
    const [limit, setLimit] = useState(8);
    const [filter, setFilter] = useState('');
    const [language, setLanguage] = useState<string | null>(null);
    const [quality, setQuality] = useState<string | null>(null);
    /** «Посмотреть ещё голоса»: весь каталог с бесконечной прокруткой открыт. */
    const [browse, setBrowse] = useState(false);
    /** «Посмотреть ещё модели» в «Распознавании». */
    const [browseAsr, setBrowseAsr] = useState(false);
    const [wizard, setWizard] = useState(false);
    /** Свой голос, которому шлюз ответил «памяти впритык»: следующее «выбрать» включает его всё равно. */
    const [tightFor, setTightFor] = useState<string | null>(null);
    const isAdmin = can('models.manage');
    const [loadError, setLoadError] = useState<string | null>(null);
    /** Шлюз без своих моделей (сборка для колонки отвечает 501): вместо карточек — пояснение. */
    const [absent, setAbsent] = useState<string | null>(null);

    useEffect(() => {
        let alive = true;
        let inFlight = false;
        let id: ReturnType<typeof setInterval> | undefined;
        const refresh = () => {
            if (inFlight) return;
            inFlight = true;
            api.models()
                .then((fresh) => {
                    if (!alive) return;
                    setView(fresh);
                    setLoadError(null);
                })
                .catch((e) => {
                    if (!alive) return;
                    if (e instanceof ApiError && e.status === 501) {
                        setAbsent(problemOf(e));
                        if (id) clearInterval(id);
                        return;
                    }
                    setLoadError(problemOf(e));
                })
                .finally(() => (inFlight = false));
        };
        void refresh();
        id = setInterval(refresh, 1500);
        return () => {
            alive = false;
            clearInterval(id);
        };
    }, []);

    // Бесконечный скролл: хвост списка виден — следующая порция карточек. Сенсор пересоздаётся
    // на каждом рендере, поэтому подписка — callback-ref, перепривязывается сама.
    const tailObserver = useRef<IntersectionObserver | null>(null);
    const observeTail = (node: HTMLParagraphElement | null): void => {
        tailObserver.current?.disconnect();
        if (!node) return;
        const observer = new IntersectionObserver(
            (entries) => {
                if (entries.some((entry) => entry.isIntersecting)) setLimit((l) => l + 8);
            },
            { rootMargin: `${NEAR_BOTTOM_PX}px` },
        );
        observer.observe(node);
        tailObserver.current = observer;
    };

    const say = (id: string, note: CardNote) => setNotes((all) => ({ ...all, [id]: note }));
    const mark = (id: string, on: boolean) => setBusy((all) => ({ ...all, [id]: on }));

    /** Карточка, о чьей модели спросило окно «нужна модель»: туда — итог. */
    const askedFor = useRef<string | null>(null);
    const needs = useNeedsFlow((message) => {
        if (askedFor.current) say(askedFor.current, { notice: message });
        void api.models().then(setView).catch(() => undefined);
    });
    /** Голосу не хватает модели или файла: окно «нужна модель», «Скачать и включить» — тот же выбор с install. */
    const askModel = (id: string, modelId: string, voiceId: string | undefined, wanted: VoiceNeeds): void => {
        const card = view?.tts.find((entry) => entry.id === id);
        askedFor.current = id;
        say(id, {});
        needs.open({
            who: card?.title ?? modelId,
            needs: wanted,
            ownId: card?.own?.id,
            retry: ({ install, anyway }) => api.activateTts(modelId, voiceId, anyway, install),
        });
    };

    /** Режим «Свой» — без Яндекса: возврат к его голосу или распознаванию сначала объясняется, второй клик проходит. */
    const guardOwn = (what: 'voice' | 'recognition', id: string, confirm: () => void): void => {
        if (view?.mode !== 'own') return confirm();
        say(id, {
            notice:
                what === 'voice'
                    ? 'У вас «Своё» — без Яндекса. Голос Яндекса вернёт его: ваши ответы будет озвучивать Яндекс. Нажмите ещё раз, чтобы согласиться.'
                    : 'У вас «Своё». Слух Яндекса уводит колонку целиком к нему (режим «Яндекс»). Нажмите ещё раз, чтобы согласиться.',
        });
    };

    const activateTts = async (id: string, modelId: string, voiceId?: string) => {
        if (!isAdmin) return;
        // Шлюз уже сказал, чего голосу не хватает: окно сразу, без лишнего запроса.
        const known = view?.tts.find((entry) => entry.id === id);
        if (known?.state === 'needs' && known.needs) return askModel(id, modelId, voiceId, known.needs);
        // Свой голос сначала поднимает сервер своей модели: это до минуты.
        say(id, modelId.startsWith('own:') ? { notice: 'Загружаю модель голоса — до минуты…' } : {});
        mark(id, true);
        try {
            const { needsRestart, pending } = await api.activateTts(modelId, voiceId, tightFor === id);
            setTightFor(null);
            if (pending) say(id, { notice: 'Голос включится сам, когда встанет его модель.' });
            else if (needsRestart) {
                // Голос скачан, но сервер ещё не знает его: без перезапуска молчит или говорит не он.
                say(id, { notice: 'Перезапускаю голосовой сервер — голос зазвучит через несколько секунд.' });
                mark('tts-server', true);
                try {
                    await api.restartServer('tts');
                } finally {
                    mark('tts-server', false);
                }
            } else {
                const title = view?.tts.find((entry) => entry.id === id)?.title;
                say(id, { notice: `Говорит ${modelId === 'yandex' ? 'Алиса (Яндекс)' : `«${title ?? modelId}»`}.` });
            }
            setView(await api.models());
        } catch (e) {
            // Не переключение, а остановка: голос не включён, кнопка на карточке становится «включить всё равно».
            const wanted = needsOf(e);
            if (isTight(e)) {
                setTightFor(id);
                say(id, { warning: `Голос не включён. ${problemOf(e)}` });
            } else if (wanted) askModel(id, modelId, voiceId, wanted);
            else say(id, { error: problemOf(e) });
        } finally {
            mark(id, false);
        }
    };

    const activateAsr = async (id: string, cardId: string) => {
        if (!isAdmin) return;
        // Слух Яндекса — это вся колонка у Яндекса (режим «Яндекс»): сначала спросить, а не уводить молча.
        if (
            cardId === 'yandex' &&
            !(await ask({
                title: 'Отдать слух Яндексу?',
                text: 'Тогда колонку целиком ведёт Яндекс: режим станет «Яндекс», свой мозг, голос и навыки замолчат. Вернуть — режимом «Вместе» или «Своё» в «Настройки» → «Колонка».',
                ok: 'Отдать Яндексу',
            }))
        )
            return;
        say(id, {});
        mark(id, true);
        try {
            if (cardId === 'yandex') {
                await api.setMode('relay');
                say(id, { notice: 'Колонка уходит к Яндексу — переподключается.' });
            } else {
                const { needsRestart } = await api.activateAsr(cardId);
                say(id, { notice: needsRestart ? 'Запомнено — перезапустите распознавание.' : 'Работает.' });
            }
            setView(await api.models());
        } catch (e) {
            say(id, { error: problemOf(e) });
        } finally {
            mark(id, false);
        }
    };

    const install = async (id: string) => {
        if (!isAdmin) return;
        try {
            await api.installVoice(id);
            setView(await api.models());
        } catch (e) {
            say(id, { error: problemOf(e) });
        }
    };

    /** Остановка скачивания: половина файла уходит с ним. */
    const cancel = async (id: string) => {
        try {
            await api.cancelVoice(id);
            setView(await api.models());
        } catch (e) {
            say(id, { error: problemOf(e) });
        }
    };

    /** Удаление скачанного голоса — не говорящего сейчас; сначала спрашиваем: его потом качать заново. */
    const uninstall = async (id: string) => {
        if (!isAdmin) return;
        const yes = await ask({ title: 'Удалить скачанный голос?', text: 'Чтобы вернуть, его придётся скачать заново.', ok: 'Удалить', danger: true });
        if (!yes) return;
        mark(id, true);
        try {
            await api.uninstallVoice(id);
            const next = await api.models();
            // Сервер, который перечитывает голоса сам, уже забыл этот; старый забудет после перезапуска.
            const still = next.tts.find((card) => card.id === id)?.loaded;
            say(id, { notice: still ? 'Удалён — голосовой сервер забудет его после перезапуска.' : 'Удалён.' });
            setView(next);
        } catch (e) {
            say(id, { error: problemOf(e) });
        } finally {
            mark(id, false);
        }
    };

    /** Файл Silero, скачанный на silero.ai: кладём его на место кнопкой. */
    const placeFile = async (id: string, file: File) => {
        if (!isAdmin) return;
        mark(id, true);
        try {
            await api.placeSileroFile(file);
            const next = await api.models();
            // Сервер, который перечитывает голоса сам, уже говорит ими; старому нужен перезапуск.
            const loaded = next.tts.find((card) => card.id === id)?.loaded;
            say(id, { notice: loaded ? 'Файл на месте — голоса работают.' : 'Файл на месте — перезапустите голосовой сервер, голоса появятся.' });
            setView(next);
        } catch (e) {
            say(id, { error: problemOf(e) });
        } finally {
            mark(id, false);
        }
    };

    /** Перезапуск сервера — кнопка в карточке, которая его попросила. */
    const restart = async (id: string, which: 'asr' | 'tts') => {
        if (!isAdmin) return;
        mark(`${which}-server`, true);
        say(id, {
            notice:
                which === 'asr'
                    ? 'Перезапускаю распознавание: сначала модель пробно загружается (новая может скачиваться минуты)…'
                    : 'Перезапускаю голосовой сервер…',
        });
        try {
            await api.restartServer(which);
            setView(await api.models());
        } catch (e) {
            say(id, { error: problemOf(e) });
        } finally {
            mark(`${which}-server`, false);
        }
    };

    if (absent) {
        return (
            <>
                {!bare && (
                    <SectionTabs
                        label="Голос и распознавание"
                        tabs={MODELS_TABS}
                        tab={tab}
                        onPick={(id) => {
                            onTab(id);
                            setLimit(8);
                        }}
                    />
                )}
                <LocalModelsAbsent kind={tab} message={absent} />
            </>
        );
    }

    if (!view) {
        return (
            <section class="card glass">
                <p class="t-meta m-0" role={loadError ? 'alert' : undefined}>
                    {loadError ?? 'Читаем каталог…'}
                </p>
            </section>
        );
    }

    const needle = filter.trim().toLowerCase();

    // Голоса под фильтрами: язык и качество из пилюль, текстовый поиск — по имени, описанию и языку.
    const voiceMatches = (card: TtsCard): boolean => {
        const code = langOf(card.locale);
        if (language && code !== language) return false;
        if (quality && card.quality !== quality) return false;
        if (!needle) return true;
        return `${card.title} ${card.description} ${card.voice ?? ''} ${LANGUAGES[code] ?? ''}`.toLowerCase().includes(needle);
    };
    // «Голос» тремя панелями: голос Алисы; «Свой голос» — снятые здесь отпечатки; готовые голоса (Silero, Piper):
    // рекомендованные и уже скачанные, а весь открытый реестр — по «Посмотреть ещё голоса», с фильтрами и прокруткой.
    const yandexCard = view.tts.find((card) => card.engine === 'yandex');
    const ownCards = view.tts.filter((card) => card.group === 'own');
    const recommendedCards = view.tts.filter((card) => card.source === 'builtin');
    const downloadedCards = view.tts.filter((card) => card.source === 'registry' && card.installed);
    const registryRest = view.tts.filter((card) => card.source === 'registry' && !card.installed);
    const moreCount = registryRest.length;
    const moreAll = registryRest.filter(voiceMatches);
    const more = moreAll.slice(0, limit);
    const refreshView = () => void api.models().then(setView).catch(() => undefined);
    const local = view.localAsr;
    /** Одна карточка распознавания: работает, ждёт перезапуска или «включить». На этом компьютере — без перезапуска:
     * выбранная модель скачивается (её вес — на карточке) и встаёт сама; модели без файла GGUF — только на сервере. */
    const asrCard = (card: AsrCard) => (
        <Card
            key={card.id}
            title={card.title}
            badge={card.badge}
            description={card.description}
            meta={
                card.local?.runsHere && !card.local.installed
                    ? [card.license, `скачается ${Math.round(card.local.size / 1_000_000)} МБ`].filter(Boolean).join(' · ')
                    : card.license
            }
            note={notes[card.id]}
            action={
                card.active ? (
                    <span class="pill pill-own">работает</span>
                ) : card.local && !card.local.runsHere && card.id !== 'yandex' ? (
                    <span class="pill pill-off" title="У этой модели нет файла для распознавания на компьютере — её запускает отдельный сервер распознавания.">
                        только на сервере
                    </span>
                ) : card.pending && local ? (
                    <span class="pill pill-off">
                        {local.phase === 'downloading'
                            ? `${Math.round(localAsrPercent(local))}%`
                            : local.phase === 'failed'
                              ? 'не встала'
                              : 'загружается'}
                    </span>
                ) : card.pending ? (
                    view.canRestart ? (
                        <Action
                            tone="on"
                            label="перезапустить"
                            busy={busy['asr-server'] || view.restarting === 'asr'}
                            onClick={() => void restart(card.id, 'asr')}
                        />
                    ) : (
                        <span class="pill pill-off">после перезапуска сервера</span>
                    )
                ) : (
                    <Action
                        label="включить"
                        onClick={() => (view.mode === 'own' && card.id === 'yandex' && !notes[card.id]?.notice ? guardOwn('recognition', card.id, () => void activateAsr(card.id, card.id)) : void activateAsr(card.id, card.id))}
                    />
                )
            }
        />
    );
    /** Одна карточка голоса: голос Алисы, рекомендованный или из реестра — с управлением справа. */
    const voiceCard = (card: TtsCard) => {
        const download = view.downloads.find((entry) => entry.id === card.id);
        const state = voicePill(card);
        return (
            <Card
                key={card.id}
                title={card.title}
                badge={card.badge}
                description={card.description}
                // Размер и лицензия: человек должен видеть, что берёт (у голосов Piper они разные).
                meta={[card.size, card.license].filter(Boolean).join(' · ') || undefined}
                note={notes[card.id]}
                pills={state ? [state] : undefined}
                action={
                    card.state === 'unsupported' && !card.active ? null : card.engine === 'yandex' ? (
                        card.active ? (
                            <span class="pill pill-own">голос Алисы</span>
                        ) : (
                            <Action
                                tone="yandex"
                                label="голос Яндекса"
                                busy={busy[card.id]}
                                onClick={() =>
                                    view.mode === 'own' && !notes[card.id]?.notice
                                        ? guardOwn('voice', card.id, () => void activateTts(card.id, 'yandex'))
                                        : void activateTts(card.id, 'yandex')
                                }
                            />
                        )
                    ) : !card.installed ? (
                        <div class="flex items-center gap-6">
                            <Action
                                label={
                                    download?.state === 'downloading'
                                        ? `${Math.round((download.bytes / Math.max(download.total, 1)) * 100)}%`
                                        : card.downloadable
                                          ? 'скачать'
                                          : 'файл вручную'
                                }
                                tone="yandex"
                                busy={download?.state === 'downloading'}
                                onClick={() => void install(card.id)}
                            />
                            {download?.state === 'downloading' && (
                                <Action label="отмена" onClick={() => void cancel(card.id)} />
                            )}
                            {/* Голос сам знает, чего ему не хватает: «выбрать» — окно «скачать и включить». */}
                            {card.state === 'needs' && card.needs && card.downloadable && download?.state !== 'downloading' && isAdmin && (
                                <Action tone="on" label="выбрать" busy={busy[card.id]} onClick={() => void activateTts(card.id, card.id)} />
                            )}
                        </div>
                    ) : !card.loaded ? (
                        // Файл есть, а сервер его ещё не загрузил (скачан после его старта):
                        // без перезапуска он молчал бы или говорил бы не он.
                        <div class="flex items-center gap-6">
                            <Action label="удалить" busy={busy[card.id]} onClick={() => void uninstall(card.id)} />
                            {view.canRestart ? (
                                <Action
                                    tone="on"
                                    label="перезапустить"
                                    busy={busy['tts-server'] || view.restarting === 'tts'}
                                    onClick={() => void restart(card.id, 'tts')}
                                />
                            ) : (
                                <span class="pill pill-off">после перезапуска сервера</span>
                            )}
                        </div>
                    ) : card.engine === 'silero' ? (
                        card.active ? (
                            <span class="pill pill-own">говорит сейчас</span>
                        ) : (
                            <Action label="удалить" busy={busy[card.id]} onClick={() => void uninstall(card.id)} />
                        )
                    ) : card.active ? (
                        <span class="pill pill-own">говорит сейчас</span>
                    ) : (
                        <div class="flex items-center gap-6">
                            <Action label="удалить" busy={busy[card.id]} onClick={() => void uninstall(card.id)} />
                            <Action
                                tone="on"
                                label="выбрать"
                                busy={busy[card.id]}
                                onClick={() => void activateTts(card.id, card.id)}
                            />
                        </div>
                    )
                }
            >
                {card.engine === 'silero' && card.installed && card.voices.length > 0 && (
                    <div class="flex flex-wrap gap-6">
                        {card.voices.map((voice) => {
                            const chosen = card.active && view.ttsActive.voiceId === voice.id;
                            return (
                                <button
                                    key={voice.id}
                                    class={chosen ? 'pill pill-own cursor-pointer bg-accent c-on-accent fw-800' : 'pill pill-off cursor-pointer'}
                                    onClick={() => void activateTts(card.id, card.id, voice.id)}
                                >
                                    {voice.name}
                                </button>
                            );
                        })}
                    </div>
                )}
                {download && download.state !== 'done' && <DownloadBar download={download} />}
                {!download && card.state === 'installing' && card.progress !== undefined && <Progress percent={card.progress} label={`Установка: ${card.title}`} />}
                {card.reason && card.state !== 'ready' && <span class="t-caption">{card.reason}</span>}
            </Card>
        );
    };
    const pending = view.ttsPending;
    const pendingTitle = pending ? (pending.title ?? view.tts.find((card) => card.id === pending.modelId)?.title ?? pending.modelId) : '';
    // «Распознавание» так же, как «Голос»: сверху то, что работает (или ждёт перезапуска); «Советуем» — GigaAM v3;
    // остальные модели — по «Посмотреть ещё модели».
    // Шлюз без признака «советуем» (до его перезапуска) — хотя бы карточка с плашкой «рекомендуем».
    const advised = (card: AsrCard): boolean => card.recommended ?? card.badge === 'рекомендуем';
    const currentAsr = view.asr.filter((card) => card.active || card.pending);
    const advisedAsr = view.asr.filter((card) => advised(card) && !currentAsr.includes(card));
    const otherAsr = view.asr.filter((card) => !advised(card) && !currentAsr.includes(card));

    // Языки, какие есть в каталоге: русский и английский первыми, дальше по количеству голосов.
    const languageCounts = new Map<string, number>();
    for (const card of view.tts) {
        const code = langOf(card.locale);
        if (code) languageCounts.set(code, (languageCounts.get(code) ?? 0) + 1);
    }
    const weight = (code: string): number => (code === 'ru' ? 0 : code === 'en' ? 1 : 2);
    const languages = [...languageCounts.entries()].sort((a, b) => weight(a[0]) - weight(b[0]) || b[1] - a[1] || a[0].localeCompare(b[0]));
    // Качества — какие есть при выбранном языке.
    const qualities = QUALITIES.filter((entry) => view.tts.some((card) => card.quality === entry.id && (!language || langOf(card.locale) === language)));

    return (
        <>
            {!bare && (
                <SectionTabs
                    label="Голос и распознавание"
                    tabs={MODELS_TABS}
                    tab={tab}
                    onPick={(id) => {
                        onTab(id);
                        setLimit(8);
                    }}
                />
            )}

            {tab === 'recognition' ? (
                <>
                    <section class="card glass card-list" aria-label="Слух">
                        <div class="card-head">
                            <h2 class="t-title-xs m-0">
                                Слух
                            </h2>
                            <span class="t-meta">
                                {view.mode === 'relay'
                                    ? 'в режиме «Яндекс» распознаёт Яндекс'
                                    : local
                                      ? localAsrLine(local)
                                      : view.asrLoaded
                                        ? `работает: ${view.asrLoaded}`
                                        : 'сервер молчит'}
                            </span>
                        </div>
                        {local?.phase === 'downloading' && <Progress percent={localAsrPercent(local)} label="Модель распознавания" />}
                        {local && (local.phase === 'failed' || local.gpuFailed) && (
                            <div class="flex items-center gap-12 py-8">
                                <span class={`t-caption flex-1 ${local.phase === 'failed' ? 'c-dot-offline' : ''}`} role={local.phase === 'failed' ? 'alert' : 'status'}>
                                    {local.phase === 'failed'
                                        ? (local.error ?? 'Распознавание не запустилось.')
                                        : 'Видеокарта отказала — распознаёт процессор. «Повторить» попробует видеокарту снова.'}
                                </span>
                                {isAdmin && (
                                    <Action
                                        tone="on"
                                        label="повторить"
                                        busy={busy['asr-server'] || view.restarting === 'asr'}
                                        onClick={() => void restart('local-asr', 'asr')}
                                    />
                                )}
                            </div>
                        )}
                        {currentAsr.map(asrCard)}
                    </section>
                    <section class="card glass card-list" aria-label="Советуем">
                        <div class="card-head">
                            <h2 class="t-title-xs m-0">
                                Советуем
                            </h2>
                            <span class="t-meta">GigaAM от Сбера — для русской речи</span>
                        </div>
                        {advisedAsr.map(asrCard)}
                        {!browseAsr ? (
                            <button
                                class="pill pill-off flex w-full justify-center cursor-pointer py-12 px-16 mt-12"
                                onClick={() => setBrowseAsr(true)}
                            >
                                Посмотреть ещё модели · {otherAsr.length}
                            </button>
                        ) : (
                            <>
                                <div class="flex items-center justify-between p-0 pt-14 pb-6">
                                    <span class="t-meta">Ещё модели · {otherAsr.length}</span>
                                    <button class="t-caption cursor-pointer underline" onClick={() => setBrowseAsr(false)}>
                                        Свернуть
                                    </button>
                                </div>
                                {otherAsr.map(asrCard)}
                            </>
                        )}
                    </section>
                </>
            ) : (
                <>
                    {pending && <PendingVoice pending={pending} title={pendingTitle} isAdmin={isAdmin} onChanged={refreshView} />}
                    {view.ttsManaged && ['installing', 'starting', 'failed'].includes(view.ttsManaged.phase) && (
                        // Сервер своего голоса, который держит приложение: ставится, запускается или упал.
                        <section class="card glass card-list" aria-label="Сервер своего голоса">
                            <div class="card-head">
                                <h2 class="t-title-xs m-0">Свой голос</h2>
                                <span class="t-meta">
                                    {view.ttsManaged.phase === 'installing'
                                        ? (view.ttsManaged.stage ?? 'ставлю')
                                        : view.ttsManaged.phase === 'starting'
                                          ? 'запускаю сервер голоса…'
                                          : 'сервер голоса не работает'}
                                </span>
                            </div>
                            {view.ttsManaged.phase === 'installing' && (
                                <Progress percent={view.ttsManaged.percent} label="Установка своего голоса" />
                            )}
                            {view.ttsManaged.phase === 'failed' && (
                                <div class="flex items-center gap-12 py-8">
                                    <span class="t-caption flex-1 c-dot-offline" role="alert">
                                        {view.ttsManaged.error ?? 'Сервер голоса не запустился.'}
                                    </span>
                                    {isAdmin && (
                                        <Action
                                            tone="on"
                                            label="повторить"
                                            busy={busy['tts-server'] || view.restarting === 'tts'}
                                            onClick={() => void restart('tts-managed', 'tts')}
                                        />
                                    )}
                                </div>
                            )}
                        </section>
                    )}
                    <section class="card glass card-list" aria-label="Голос Алисы">
                        <div class="card-head">
                            <h2 class="t-title-xs m-0">
                                Голос
                            </h2>
                            <span class="t-meta">
                                {view.mode === 'relay'
                                    ? 'в режиме «Яндекс» говорит Алиса'
                                    : view.speaking === 'own' && !view.ttsUp
                                      ? 'голосовой сервер молчит'
                                      : 'говорит: '}
                                {view.mode === 'relay' || (view.speaking === 'own' && !view.ttsUp)
                                    ? ''
                                    : view.ttsActive.modelId === 'yandex'
                                      ? 'Алиса (Яндекс)'
                                      : view.tts.find((card) => card.active && card.engine !== 'yandex')?.title ?? (view.ttsActive.modelId || '—')}
                            </span>
                        </div>
                        {yandexCard && voiceCard(yandexCard)}
                        {view.mode === 'relay' && (
                            <p
                                role="note"
                                class="m-0 mt-10 mb-2 py-8 px-12 r-pill bg-pill-yandex-bg c-pill-yandex-fg fs-13 lh-18"
                            >
                                Сейчас режим «Яндекс» — говорит голос Алисы. Свой голос можно выбрать заранее: он зазвучит,
                                когда вернётесь в «Вместе» или «Своё».
                            </p>
                        )}
                    </section>
                    <section class="card glass card-list" aria-label="Свой голос">
                        <OwnVoicesHeader isAdmin={isAdmin} hasOwn={ownCards.length > 0} onAdd={() => setWizard(true)} />
                        <OwnVoiceRows
                            cards={ownCards}
                            isAdmin={isAdmin}
                            notes={notes}
                            busy={busy}
                            onActivate={(cardId) => void activateTts(cardId, cardId)}
                            tightFor={tightFor}
                            onChanged={refreshView}
                        />
                    </section>
                    <section class="card glass card-list" aria-label="Готовые голоса">
                        <div class="card-head">
                            <h2 class="t-title-xs m-0">
                                Готовые голоса
                            </h2>
                            <span class="t-meta">Silero и Piper</span>
                        </div>
                        <p class="t-caption m-0 mb-6">
                            Рекомендованные и уже скачанные. Остальные — в каталоге ниже.
                        </p>
                        {recommendedCards.map(voiceCard)}
                        {downloadedCards.map(voiceCard)}
                        {!browse ? (
                            <button
                                class="pill pill-off flex w-full justify-center cursor-pointer py-12 px-16 mt-12"
                                onClick={() => {
                                    setBrowse(true);
                                    setLimit(8);
                                }}
                            >
                                Посмотреть ещё голоса · {moreCount}
                            </button>
                        ) : (
                            <>
                                <div class="flex items-center justify-between p-0 pt-14 pb-6">
                                    <span class="t-meta">Ещё голоса · {moreCount}</span>
                                    <button class="t-caption cursor-pointer underline" onClick={() => setBrowse(false)}>
                                        Свернуть
                                    </button>
                                </div>
                                <div class="flex items-center gap-8 p-0 pt-2 pb-8 flex-wrap">
                                    {languages.length > 1 && (
                                        <Dropdown
                                            label="Язык"
                                            anyLabel="Все языки"
                                            value={language}
                                            options={languages.map(([code, count]) => ({
                                                id: code,
                                                label: LANGUAGES[code] ?? code.toUpperCase(),
                                                hint: String(count),
                                            }))}
                                            onPick={(code) => {
                                                setLanguage(code);
                                                setLimit(8);
                                            }}
                                        />
                                    )}
                                    {qualities.length > 1 && (
                                        <Dropdown
                                            label="Качество"
                                            anyLabel="Любое"
                                            value={quality}
                                            options={qualities.map((entry) => ({ id: entry.id, label: entry.label }))}
                                            onPick={(id) => {
                                                setQuality(id);
                                                setLimit(8);
                                            }}
                                        />
                                    )}
                                    {
                                        <input
                                            value={filter}
                                            onInput={(e) => {
                                                setFilter((e.target as HTMLInputElement).value);
                                                setLimit(8);
                                            }}
                                            placeholder="Поиск: язык, имя"
                                            aria-label="Поиск голоса"
                                            class="flex-1-1-160 min-w-0 py-8 px-14 r-pill border-glass bg-icon-bg outline-none"
                                        />
                                    }
                                </div>
                                {more.map(voiceCard)}
                                {more.length < moreAll.length && (
                                    <p ref={observeTail} class="t-caption m-0 mt-8 text-center">
                                        Ещё {moreAll.length - more.length} — прокрутите
                                    </p>
                                )}
                                {moreAll.length === 0 && (
                                    <p class="t-caption my-8 mx-0 text-center">
                                        Ничего не нашлось — сбросьте фильтры или поиск.
                                    </p>
                                )}
                            </>
                        )}
                    </section>
                </>
            )}

            {!isAdmin && (
                <p class="t-caption my-0 mx-6">Скачивать и переключать модели может администратор.</p>
            )}
            {!view.ownVoiceServer && tab === 'voice' && (
                <p class="t-caption my-0 mx-6">Свой голосовой сервер не настроен (TTS_OWN_URL): выбрать можно только голос Алисы.</p>
            )}
            {wizard && (
                <VoiceWizard
                    onClose={() => {
                        setWizard(false);
                        refreshView();
                    }}
                />
            )}
            {needs.dialog}
        </>
    );
}
