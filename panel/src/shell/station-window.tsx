// Окно колонки у помощника (docs/plans/desk.md, этап 4; макеты WindowBrain, HelperOffline): снаружи — рамка окна на
// столе помощника с пультом колонки во фрейме (StationFrame); внутри фрейма — тот же пульт в режиме `?via=` (via.ts)
// с полосой колонки сверху (WindowShell): её имя, её шлюз, на связи ли, «изменения сохраняются на колонке». Колонки
// нет на связи — полоса говорит, от какого времени копия и что менять можно, когда колонка вернётся. Несохранённое во
// фрейме (мастерская колонки) спрашивает раньше, чем помощник закроет окно — ×, доком или Esc (shell/leave.ts). Шит
// во фрейме затемняет весь экран помощника, не только фрейм (useFrameSheet). Своего живого канала у пульта во фрейме
// нет: канал колонки пересказывает шлюз компьютера по сокету страницы, а страница передаёт его фрейму (useFrameLive).

import type { RefObject } from 'preact';
import { createPortal } from 'preact/compat';
import { useContext, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { LiveMessage, stationCall, viaRefusal } from '../api';
import { STATION_LIVE, STATION_READY, onStationMessage, stationHello } from '../live';
import { usePoll } from '../poll';
import { DISMISS_MESSAGE, SHEET_MESSAGE } from '../ui/sheet';
import { usePanel } from './context';
import { exposeLeave, frameMayLeave, useLeaveGuard } from './leave';
import { hashOf } from './links';
import { ownName } from './role';
import type { ShellProps } from './shell';
import { ESCAPE_MESSAGE, WindowLift, escapeIsFree } from './window';
import { copyShown, onCopyChange } from '../via';

const hhmm = (at: number): string => {
    const date = new Date(at);
    const time = date.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    return date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString('ru-RU')} ${time}`;
};

function useCopy(): number | null {
    const [, rerender] = useState(0);
    useEffect(() => onCopyChange(() => rerender((n) => n + 1)), []);
    return copyShown();
}

/**
 * Пульт колонки во всё окно (docs/plans/panel-rebuild.md, этап 4): колонка не отвечает — плашка, что показана её
 * копия и что менять можно, когда она вернётся. «Компьютер-помощник» при этом работает: он говорит с компьютером сам.
 */
export function ViaNotice() {
    const copy = useCopy();
    if (copy === null) return null;
    return (
        <div class="banner-offline" role="status">
            Колонка не отвечает — показываю её копию от {hhmm(copy)}. Менять можно, когда она вернётся; «Компьютер-помощник» в
            «Настройках» работает.
        </div>
    );
}

/** Полоса колонки над её разделом: чья она, на связи ли, где сохраняется. */
function StationStrip() {
    const panel = usePanel();
    const copy = useCopy();
    const name = ownName(panel.health, panel.role) ?? panel.current?.name ?? panel.drawing.title;
    const version = panel.health?.build?.version;
    const away = copy !== null || panel.offline;
    return (
        <div class={`station-strip${away ? ' is-away' : ''}`} role="status">
            <span class="station-strip-name">Колонка «{name}»</span>
            <span class="w-cap">шлюз{version ? ` · DuoWave ${version}` : ''}</span>
            <span class={`role-chip${away ? ' is-off' : ''}`}>
                <span class="role-chip-dot" aria-hidden="true" />
                {away ? 'не на связи' : 'на связи'}
            </span>
            <span class="station-strip-note">
                {away
                    ? `Показываю копию${copy ? ` от ${hhmm(copy)}` : ''}. Менять можно, когда колонка вернётся — тогда изменения сразу уйдут на неё.`
                    : 'Изменения сохраняются на колонке и сразу работают.'}
            </span>
        </div>
    );
}

/**
 * Esc во фрейме до страницы помощника не доходит: ничей Esc (не поле, не список, не шит) пульт колонки передаёт ей
 * словом, и она закрывает окно (shell/window.tsx). О несохранённом здесь спросит она — через вопрос фрейма
 * (useLeaveToHelper), один раз, как и на ×.
 */
function useEscapeToHelper(): void {
    useEffect(() => {
        if (window.parent === window) return;
        const onKey = (event: KeyboardEvent) => {
            if (!escapeIsFree(event)) return;
            event.preventDefault();
            window.parent.postMessage({ type: ESCAPE_MESSAGE }, location.origin);
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, []);
}

/** Пульт колонки во фрейме отвечает странице помощника, можно ли закрыть окно (StationFrame спрашивает). */
function useLeaveToHelper(): void {
    useEffect(() => {
        if (window.parent === window) return;
        return exposeLeave();
    }, []);
}

/** Пульт колонки в окне помощника: полоса колонки и открытый раздел, без своего дока. */
export function WindowShell({ notice, desk, window: win }: ShellProps) {
    useEscapeToHelper();
    useLeaveToHelper();
    return (
        <div class="station-window">
            <StationStrip />
            {notice}
            <div class="station-window-body">{win ? win.content : desk}</div>
        </div>
    );
}

const REFUSALS = {
    'signed-out': {
        title: 'Вход в пульт компьютера закончился',
        text: 'Войдите в пульт этого компьютера заново — окно колонки откроется с тем же входом.',
    },
    pair: {
        title: 'Колонка не принимает ключ пары',
        text: 'Похоже, колонку переустановили или сбросили, и ключ у неё теперь другой. Свяжите с ней компьютер заново — по паролю пульта колонки.',
    },
    setup: {
        title: 'Шлюз колонки ещё не настроен',
        text: 'Откройте пульт колонки по её адресу и задайте вход — потом её разделы откроются и здесь.',
    },
    other: {
        title: 'Окно колонки не открылось',
        text: 'Попробуйте ещё раз чуть позже.',
    },
};

/**
 * Окно колонки, которое не пустили (docs/plans/desk.md, этап 4). Вход здесь не спрашивается: он у пульта компьютера, а
 * пароль колонки окну ни к чему. Окно говорит, в чём дело: кончился вход компьютера или колонка не узнала ключ пары.
 */
export function ViaRefused({ setup, top = false }: { setup: boolean; top?: boolean }) {
    useEscapeToHelper();
    const [why, setWhy] = useState<{ kind: keyof typeof REFUSALS; message?: string } | null>(setup ? { kind: 'setup' } : null);
    useEffect(() => {
        if (setup) return;
        let alive = true;
        void viaRefusal().then((found) => alive && setWhy(found));
        return () => {
            alive = false;
        };
    }, [setup]);
    const said = why && REFUSALS[why.kind];
    return (
        <div class="station-window">
            <div class="station-window-body">
                <section class="card glass flex flex-col gap-10" role="alert">
                    {said ? (
                        <>
                            <h2 class="t-title-xs m-0">{said.title}</h2>
                            <p class="t-caption m-0">{why?.message ?? said.text}</p>
                            <div class="flex gap-8 flex-wrap">
                                <button class="w-btn" onClick={() => location.reload()}>
                                    Попробовать снова
                                </button>
                                {/* Во всё окно (этап 4) — путь в пульт самого компьютера: войти, связать заново. */}
                                {top && (
                                    <button class="w-btn" onClick={() => location.replace(`${location.pathname}?local${location.hash}`)}>
                                        Открыть пульт компьютера
                                    </button>
                                )}
                            </div>
                        </>
                    ) : (
                        <p class="t-caption m-0">Колонка не пустила — выясняю почему…</p>
                    )}
                </section>
            </div>
        </div>
    );
}

/**
 * Шит во фрейме открыт (пульт колонки сказал SHEET_MESSAGE, ui/sheet.tsx): его затемнение закрывает только фрейм, а
 * весь экран должен потемнеть, как у шита на странице. Остальная страница помощника — верхняя строка, док, стол —
 * затемняется здесь, на слое шитов (40), а окно с фреймом встаёт над этим затемнением (shell/window.tsx, WindowLift),
 * чтобы шит во фрейме был виден. Нажатие на затемнение и Esc на странице помощника уходят во фрейм словом
 * DISMISS_MESSAGE: там закрывается верхний шит, как от нажатия мимо него. Слова — только от своего фрейма и с этого же
 * адреса. Фрейм ушёл или перезагрузился — он сам говорит, что шитов нет (ui/sheet.tsx), а без фрейма нет и затемнения.
 */
function useFrameSheet(frame: RefObject<HTMLIFrameElement>): { open: boolean; dismiss: () => void } {
    const [open, setOpen] = useState(false);
    const lift = useContext(WindowLift);
    const dismiss = () => frame.current?.contentWindow?.postMessage({ type: DISMISS_MESSAGE }, location.origin);
    // Слушать раньше, чем фрейм успеет загрузиться и сказать.
    useLayoutEffect(() => {
        const onMessage = (event: MessageEvent) => {
            if (event.origin !== location.origin || !frame.current || event.source !== frame.current.contentWindow) return;
            const data = event.data as { type?: unknown; open?: unknown } | null;
            if (data?.type === SHEET_MESSAGE) setOpen(data.open === true);
        };
        window.addEventListener('message', onMessage);
        return () => window.removeEventListener('message', onMessage);
    }, []);
    // Esc на странице помощника (фокус ушёл из фрейма) — шиту фрейма, а не окну: окно его не берёт, пока затемнено.
    useEffect(() => {
        if (!open) return;
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== 'Escape' || event.defaultPrevented) return;
            event.preventDefault();
            dismiss();
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [open]);
    // Окно стола — над затемнением, его заголовок и рамка — под вуалью; в боковой панели окна нет, встаёт сам фрейм.
    // В том же кадре, что затемнение (useLayoutEffect): иначе кадр окно с шитом затемнено дважды, а при закрытии кадр
    // стоит поднятым над уже светлой страницей.
    useLayoutEffect(() => {
        if (!lift || !open) return;
        lift(dismiss);
        return () => lift(null);
    }, [open, lift]);
    return { open, dismiss };
}

/**
 * Канал колонки — её пульту во фрейме. Своего канала у фрейма нет: шлюз компьютера пересказывает канал колонки по сокету
 * этой страницы (live.ts), а страница передаёт каждое его сообщение фрейму словом STATION_LIVE. Фрейм загрузился и
 * сказал «слушаю» (STATION_READY) — ему то, что известно о колонке сейчас, сообщением hello. Слова — только своему
 * фрейму и от него, с этого же адреса.
 */
function useFrameLive(frame: RefObject<HTMLIFrameElement>, station: string): void {
    // Слушать раньше, чем фрейм успеет загрузиться и сказать «слушаю».
    useLayoutEffect(() => {
        const send = (message: LiveMessage | null): void => {
            frame.current?.contentWindow?.postMessage({ type: STATION_LIVE, message }, location.origin);
        };
        const onReady = (event: MessageEvent) => {
            if (event.origin !== location.origin || !frame.current || event.source !== frame.current.contentWindow) return;
            if ((event.data as { type?: unknown } | null)?.type !== STATION_READY) return;
            const hello = stationHello(station);
            if (hello) send(hello);
        };
        window.addEventListener('message', onReady);
        const stop = onStationMessage((from, message) => from === station && send(message));
        return () => {
            window.removeEventListener('message', onReady);
            stop();
        };
    }, [station]);
}

/**
 * Раздел колонки во фрейме окна помощника: тот же пульт через проход компьютера. Уход отсюда — закрыть окно (×, Esc),
 * другой раздел в доке — сначала спрашивает фрейм: у мастерской колонки может быть несохранённый файл. Вкладка и её
 * элемент — в адресе фрейма: `#/plugins/workshop/<черновик>` открывает черновик в мастерской колонки.
 */
export function StationFrame({ station, section, tab, item }: { station: string; section: string; tab?: string; item?: string }) {
    const frame = useRef<HTMLIFrameElement>(null);
    useLeaveGuard(true, () => frameMayLeave(frame.current));
    const sheet = useFrameSheet(frame);
    useFrameLive(frame, station);
    // Пульт живёт под /panel/ и на шлюзе, и в разработке (vite.config.ts: base).
    const hash = hashOf({ section, tab, item: tab ? item : undefined, query: {} });
    const src = `/panel/?via=${encodeURIComponent(station)}&window=${encodeURIComponent(section)}${hash}`;
    return (
        <>
            <iframe
                ref={frame}
                class={`station-frame desk-window-wide${sheet.open ? ' is-lifted' : ''}`}
                src={src}
                title={`Пульт колонки: ${section}`}
            />
            {sheet.open &&
                createPortal(
                    <button class="frame-scrim" aria-label="Закрыть окно поверх" tabIndex={-1} onClick={sheet.dismiss} />,
                    document.body,
                )}
        </>
    );
}

const MODE_NAMES: Record<string, string> = { relay: 'Яндекс', gateway: 'Вместе', own: 'Своё' };

/**
 * «у колонки «Свой»» в верхней строке помощника: режим колонки — у её шлюза, через проход. Раз в полминуты и по
 * переменам, без наложений (poll.ts).
 */
export function StationModeChip({ station, version }: { station: string; version: number }) {
    const [mode, setMode] = useState<string | null>(null);
    usePoll({
        deps: [station],
        every: 30_000,
        version,
        read: (signal) =>
            stationCall<{ mode: string }>(station, 'GET', '/api/v1/station/mode', undefined, signal)
                .then((view) => !signal.aborted && setMode(view.mode))
                .catch(() => !signal.aborted && setMode(null)),
    });
    if (!mode) return null;
    return (
        <span class="role-chip">
            <span class={`role-chip-dot ${mode === 'own' ? 'own' : mode === 'relay' ? 'yandex' : 'gateway'}`} aria-hidden="true" />
            у колонки «{MODE_NAMES[mode] ?? mode}»
        </span>
    );
}
