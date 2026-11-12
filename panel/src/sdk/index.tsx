// SDK пульта для плагинов (docs/plans/plugins.md, этап 5; типы для авторов — docs/plugins/panel-sdk.d.ts). Модуль
// плагина (`panel.js` пакета) импортирует «preact», «preact/hooks» и «@duowave/panel-sdk» — import map в index.html ведёт
// их к файлам public/sdk/, а те берут всё отсюда: у плагина нет своей копии Preact, и его компоненты живут в дереве
// пульта, в своих границах ошибок. Плагин ходит в шлюз только через useApi — к функциям своего бэкенда, которые его
// манифест перечислил в contributes.panel.calls; права проверяет шлюз.

import * as preact from 'preact';
import type { ComponentChildren } from 'preact';
import * as hooks from 'preact/hooks';
import * as jsxRuntime from 'preact/jsx-runtime';

import { DraftTrying as DraftTryingValue, PackagesView, PanelMode, RequestView, StationPresence, api, can, fetchPluginFile, stationCall } from '../api';
import { IconCoins, IconGlobe, IconHeart, IconList, IconMusic, IconPackage, IconSparkle, IconSpeaker, IconTimer, IconWeather } from '../icons';
import { enabledHere, packageSettings } from '../plugins/loader';
import { PluginScope, usePluginVersion } from '../plugins/versions';
import { PanelContext, usePanel } from '../shell/context';
import { definePlugin, onRegistryChange } from '../shell/registry';
import { ownName } from '../shell/role';
import { Action } from '../ui/cards';
import { ask } from '../ui/confirm';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { VIA } from '../via';
import { melodyOf, noteName, readMidiFile, useMidi } from './midi';
import { PianoKeys } from './piano-keys';
import { useSynth } from './synth';

/**
 * Виджет черновика, показанный до установки («+» → «Новый виджет», мастерская; docs/plans/desk.md, этап 5): его
 * бэкенд зовётся в песочнице мастерской, а не у поставленного пакета. Метка — у самого предпросмотра (контекст его
 * дерева), а не на весь пульт: поставленный плагин с тем же id на столе зовёт свой бэкенд, и закрытый предпросмотр
 * ничего за собой не оставляет.
 */
export const DraftPreview = preact.createContext<string | null>(null);

/**
 * Как предпросмотр черновика зовёт песочницу (docs/plans/voice-creation.md): плитка на столе и «Примерка» — всегда на
 * примере (`examples.json`, до «да» черновик в сеть не ходит) и с обычными настройками, что человек выбрал в
 * «Примерке». Мастерская — как раньше, по сети.
 */
export const DraftTrying = preact.createContext<DraftTryingValue>({});

async function callDraft<T>(id: string, fn: string, input: unknown, trying: DraftTryingValue): Promise<T> {
    const report = await api.runDraft(id, fn, input, trying);
    if (!report.ok) throw new Error(report.error ?? `${fn} не отработала`);
    return report.result as T;
}

/** Список пакетов колонки держится столько, потом читается снова: пакеты там меняют и мимо этого пульта. */
const STATION_PACKAGES_MS = 120_000;
/** Не прочитался (колонки нет на связи и копии нет, нет права) — до новой попытки. */
const STATION_PACKAGES_RETRY_MS = 30_000;

/** Пакет на колонке: включён ли он там и как называется. */
interface StationPackage {
    enabled: boolean;
    title: string;
}

/** Пакеты, что стоят на колонке помощника, по её ключу: когда прочитан список и что в нём (null — не прочитать). */
const stationPackages = new Map<string, { at: number; list: Promise<Map<string, StationPackage> | null> }>();

/** Какие пакеты стоят на колонке и включены ли — через проход, раз на время (копия, если колонки нет на связи). */
function packagesOnStation(station: string): Promise<Map<string, StationPackage> | null> {
    const known = stationPackages.get(station);
    if (known && Date.now() - known.at < STATION_PACKAGES_MS) return known.list;
    const entry = {
        at: Date.now(),
        list: stationCall<PackagesView>(station, 'GET', '/api/v1/packages')
            .then((view) => new Map(view.packages.map((item) => [item.id, { enabled: item.enabled, title: item.title }])))
            .catch(() => {
                entry.at = Date.now() - STATION_PACKAGES_MS + STATION_PACKAGES_RETRY_MS;
                return null;
            }),
    };
    stationPackages.set(station, entry);
    return entry.list;
}

/**
 * Пакеты поменялись (packages.changed — пульт перечитал свои плагины; мастерская поставила черновик на колонку):
 * список пакетов колонки прочитается заново при следующем вызове.
 */
export function forgetStationPackages(): void {
    stationPackages.clear();
}
onRegistryChange(forgetStationPackages);

/**
 * Вызов бэкенда плагина. У помощника голос колонки обслуживает её шлюз — и плагин на колонке (его хранилище, события
 * её плеера): виджет плагина, который включён на колонке, зовёт его через проход компьютера. Выключен там — свой шлюз,
 * если плагин включён на компьютере (виджет, сделанный через «+» у помощника, — тоже так); нет и здесь — виджет говорит,
 * что плагин на колонке выключен. Список колонки не прочитать — свой шлюз.
 */
async function callPlugin<T>(id: string, fn: string, input: unknown, station: string | undefined): Promise<T> {
    if (station) {
        const there = (await packagesOnStation(station))?.get(id);
        if (there?.enabled) {
            const path = `/api/v1/packages/${encodeURIComponent(id)}/call/${encodeURIComponent(fn)}`;
            return (await stationCall<{ result: T }>(station, 'POST', path, input)).result;
        }
        if (there && !enabledHere(id)) throw new Error(`Приложение «${there.title || id}» выключено на колонке.`);
    }
    return api.callPackage<T>(id, fn, input);
}

/**
 * Бэкенд своего плагина: `call('stations', { query })` — функция из contributes.panel.calls. У помощника — плагин
 * колонки, если он там стоит, иначе — компьютера (callPlugin). Черновик в предпросмотре — всегда песочница мастерской.
 */
export function useApi(pluginId: string): { call: <T = unknown>(fn: string, input?: unknown) => Promise<T> } {
    const draft = hooks.useContext(DraftPreview);
    const trying = hooks.useContext(DraftTrying);
    // Без пульта вокруг (не должно быть, но плагину не падать из-за этого) — свой шлюз, как раньше.
    const panel = hooks.useContext(PanelContext);
    const station = !VIA && panel?.role.role === 'helper' ? panel.role.station?.key : undefined;
    const settings = JSON.stringify(trying.settings ?? {});
    return hooks.useMemo(
        () => ({
            call: <T,>(fn: string, input: unknown = {}) =>
                draft === pluginId ? callDraft<T>(pluginId, fn, input, trying) : callPlugin<T>(pluginId, fn, input, station),
        }),
        [pluginId, draft, station, trying.example, settings],
    );
}

/**
 * Картинка своего плагина (`host.files.download`, plugins/files.rs) для `<img src>`: адрес в памяти пульта — шлюз
 * отдаёт её только с ключом входа, открытой ссылки нет. Пока не пришла или её нет — null.
 */
export function useFile(name: string): string | null {
    const plugin = hooks.useContext(PluginScope);
    const [url, setUrl] = hooks.useState<string | null>(null);
    const own = usePluginVersion(plugin);
    hooks.useEffect(() => {
        if (!plugin || !name) return;
        let alive = true;
        let made: string | null = null;
        fetchPluginFile(plugin, name)
            .then((blob) => {
                if (!alive) return;
                made = URL.createObjectURL(blob);
                setUrl(made);
            })
            .catch(() => alive && setUrl(null));
        return () => {
            alive = false;
            if (made) URL.revokeObjectURL(made);
        };
    }, [plugin, name, own]);
    return url;
}

/** Значения настроек своего плагина (меняет их администратор в окне плагина): только чтение. */
export function useSettings(pluginId: string): Record<string, unknown> {
    const panel = usePanel();
    return hooks.useMemo(() => ({ ...(packageSettings.get(pluginId) ?? {}) }), [pluginId, panel.version]);
}

/**
 * Что плагину можно знать о пульте: номер перемены (перечитать своё) и переход в раздел. Номер — свой у каждого плагина
 * (plugins/versions.ts): растёт с переменами его пакета — его данные, его настройки, его включение — не чаще раза в 5 с,
 * а не с каждой переменой в пульте. Чей компонент — пульт знает сам: каждый вклад плагина он рисует в его области.
 */
export function usePanelState(): { version: number; go: (section: string, tab?: string, item?: string) => void } {
    const panel = usePanel();
    const plugin = hooks.useContext(PluginScope);
    const draft = hooks.useContext(DraftPreview);
    const scope = plugin ?? draft;
    const own = usePluginVersion(scope);
    return { version: scope ? own : panel.version, go: (section, tab, item) => panel.go(section, tab, item) };
}

/**
 * Живой канал пульта — только чтение: открыт ли он, запросы к колонкам (слова — тому, у кого право на журнал: шлюз
 * их не пришлёт иначе) и как колонки слышно. Виджету стола — показать «что сейчас», не спрашивая шлюз.
 */
export function useLive(): { connected: boolean; requests: RequestView[]; presence: Record<string, StationPresence> } {
    const { feed } = usePanel();
    return hooks.useMemo(
        () => ({ connected: feed.connected, requests: [...feed.requests], presence: { ...feed.presence } }),
        [feed.connected, feed.requests, feed.presence],
    );
}

/** Выбранная колонка — только чтение: устройство, имя, на связи ли, режим и роль пульта (шлюз или помощник). */
export function useStation(): { device: string | null; name: string; online: boolean; mode: PanelMode | null; role: 'gateway' | 'helper' } {
    const panel = usePanel();
    return {
        device: panel.current?.device ?? null,
        name: ownName(panel.health, panel.role) ?? panel.current?.name ?? panel.drawing.title,
        online: panel.online,
        mode: panel.health?.panelMode ?? null,
        role: panel.role.role,
    };
}

/** Значки пульта для шапки виджета и раздела. */
export const icons = {
    weather: IconWeather,
    music: IconMusic,
    timer: IconTimer,
    list: IconList,
    coins: IconCoins,
    globe: IconGlobe,
    heart: IconHeart,
    speaker: IconSpeaker,
    sparkle: IconSparkle,
    package: IconPackage,
};

/** Карточка с заголовком (на столе её рамку даёт стол: виджет рисует только содержимое). */
export function Card({ title, meta, children }: { title?: string; meta?: string; children?: ComponentChildren }) {
    return (
        <section class="card glass card-list">
            {(title || meta) && (
                <div class="card-head">
                    {title && <h2 class="t-title-xs m-0">{title}</h2>}
                    {meta && <span class="t-meta">{meta}</span>}
                </div>
            )}
            {children}
        </section>
    );
}

/** Подпись: обычная, предупреждение или ошибка. */
export function Note({ tone = 'meta', children }: { tone?: 'meta' | 'warning' | 'error'; children?: ComponentChildren }) {
    const cls = tone === 'error' ? 'c-dot-offline' : tone === 'warning' ? 'c-pill-yandex-fg' : '';
    return <p class={`t-caption m-0 ${cls}`}>{children}</p>;
}

/** Метка: own — наше, yandex — Яндекс, agent — агент, off — нейтральная. */
export function Pill({ tone = 'off', children }: { tone?: 'own' | 'yandex' | 'agent' | 'off'; children?: ComponentChildren }) {
    return <span class={`pill pill-${tone}`}>{children}</span>;
}

export const sdk = {
    definePlugin,
    useApi,
    useSettings,
    useFile,
    usePanelState,
    useLive,
    useStation,
    icons,
    can,
    Card,
    Action,
    Switch,
    Sheet,
    Note,
    Pill,
    ask,
    // MIDI-клавиатура для приложений (docs/plans/midi.md, Host API 1.3).
    useMidi,
    useSynth,
    PianoKeys,
    readMidiFile,
    melodyOf,
    noteName,
};

declare global {
    // eslint-disable-next-line no-var
    var __duowave: { preact: typeof preact; hooks: typeof hooks; jsxRuntime: typeof jsxRuntime; sdk: typeof sdk } | undefined;
}

/** Отдать плагинам Preact и SDK пульта — до загрузки первого panel.js. */
export function installSdk(): void {
    globalThis.__duowave = { preact, hooks, jsxRuntime, sdk };
}
