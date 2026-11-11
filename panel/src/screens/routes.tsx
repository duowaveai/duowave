import { useEffect, useRef, useState } from 'preact/hooks';
import { devMode } from '../shell/dev';

import { AccessCatalog, Health, RoutesSetting, api, can, plural, problemOf } from '../api';
import { CAPABILITIES, CapabilityMeta, capabilityIcon } from '../capabilities';
import { capabilityIconComponent } from '../icons';
import { Overlay } from '../ui/sheet';

function routePill(route: string | undefined): { cls: string; label: string } {
    switch (route) {
        case 'own':
            return { cls: 'pill-own', label: 'свой' };
        case 'own,yandex':
            return { cls: 'pill-own', label: 'свой + Яндекс' };
        case 'yandex':
            return { cls: 'pill-yandex', label: 'Яндекс' };
        case 'yandex,own':
            return { cls: 'pill-yandex', label: 'Яндекс + свой' };
        default:
            return { cls: 'pill-off', label: route === 'off' ? 'выкл' : '—' };
    }
}

/** Что можно выбрать в шите: значения те же, что понимает ROUTES. */
const OPTIONS: Array<{ value: string; label: string; cls: string; note: string }> = [
    { value: 'own', label: 'свой', cls: 'pill-own', note: 'отвечает только наш код' },
    { value: 'own,yandex', label: 'свой + Яндекс', cls: 'pill-own', note: 'сначала наш, незнакомое подхватит Яндекс' },
    { value: 'yandex', label: 'Яндекс', cls: 'pill-yandex', note: 'отдаём Яндексу целиком' },
    { value: 'off', label: 'выкл', cls: 'pill-off', note: 'никто: просьба попадёт в «Пока не умеем»' },
];

interface Props {
    health: Health | null;
    catalog: AccessCatalog | null;
    /** Заголовок списка: в «Навыках» — «Что умеет колонка»; кто отвечает — у каждой строки. */
    title?: string;
}

/** Возможности — из каталога шлюза (новая видна сразу), иконки — пульта. */
function capabilitiesOf(catalog: AccessCatalog | null): CapabilityMeta[] {
    if (!catalog) return CAPABILITIES;
    return catalog.capabilities.map((capability) => ({
        name: capability.id,
        title: capability.title,
        icon: capabilityIcon(capability.id),
    }));
}

export function RoutesScreen({ health, catalog, title = 'Кто отвечает' }: Props) {
    const served = health?.skills?.served;
    const implemented = health?.skills?.implemented;
    const mayEdit = can('routes.edit');

    const [filter, setFilter] = useState('');
    const [sheet, setSheet] = useState<CapabilityMeta | null>(null);
    /** Выбранное, но не применённое: «станет … вместо …» ждёт «Применить». */
    const [pending, setPending] = useState<string | null>(null);
    const [setting, setSetting] = useState<RoutesSetting | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [saving, setSaving] = useState(false);
    const firstOption = useRef<HTMLButtonElement>(null);

    const close = (): void => {
        setSheet(null);
        setPending(null);
    };

    // Маршруты — из API колонки (он отвечает и в режиме «Яндекс», чтобы настроить заранее); счётчики — из /health.
    useEffect(() => {
        api.routes()
            .then(setSetting)
            .catch(() => setSetting(null));
    }, [health]);

    // Шит: фокус внутрь, Esc закрывает.
    useEffect(() => {
        if (!sheet) return;
        firstOption.current?.focus();
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== 'Escape' || event.defaultPrevented) return;
            event.preventDefault();
            close();
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [sheet]);

    const routes = setting?.routes ?? health?.skills?.routes;
    const isDefault = setting?.isDefault ?? null;
    const own = health?.panelMode === 'own';

    if (!routes) {
        return (
            <section class="card glass flex flex-col gap-12">
                <h2 class="t-title-xs m-0">
                    {title}
                </h2>
                <p class="t-meta m-0">
                    Ждём шлюз…
                </p>
            </section>
        );
    }

    const needle = filter.trim().toLowerCase();
    const rows = capabilitiesOf(catalog).filter((c) => !needle || c.title.toLowerCase().includes(needle));

    /** Переключение уходит на шлюз; пилюля обновится сама, из опроса /health (≤3 с). */
    const apply = async (capability: CapabilityMeta, value: string) => {
        setSaving(true);
        setError(null);
        try {
            const result = await api.setRoutes({ ...routes, [capability.name]: value });
            setSetting({ routes: result.routes, isDefault: false });
            close();
            if (result.problems.length) setError(result.problems.join('; '));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setSaving(false);
        }
    };

    return (
        <>
            <section class="card glass card-list card-list-tight" aria-labelledby="routes-h">
                <div class="flex items-center gap-12">
                    <h2 id="routes-h" class="t-title-xs m-0 flex-1">
                        {title}
                    </h2>
                    <input
                        value={filter}
                        onInput={(e) => setFilter((e.target as HTMLInputElement).value)}
                        placeholder="Поиск"
                        aria-label="Поиск по возможностям"
                        class="field flex-none w-132"
                    />
                </div>
                {rows.map((capability) => {
                    const route = routes[capability.name];
                    const pill = routePill(route);
                    const counts = served?.[capability.name];
                    const mine = counts?.own ?? 0;
                    const yandex = counts?.yandex ?? 0;
                    const hasOwn = implemented?.[capability.name] === true;
                    const Icon = capabilityIconComponent(capability.icon);
                    return (
                        <button
                            key={capability.name}
                            class="route-row w-full text-left flex"
                            onClick={() => setSheet(capability)}
                            aria-label={`${capability.title}: ${pill.label}${mayEdit ? ', изменить' : ''}`}
                        >
                            <span class="route-icon" aria-hidden="true">
                                <Icon size={18} />
                            </span>
                            <div class="flex-1 min-w-0 flex flex-col">
                                <span class="fw-700">{capability.title}</span>
                                <span class="t-caption overflow-hidden text-ellipsis nowrap">
                                    {!served
                                        ? '—'
                                        : mine + yandex > 0
                                          ? `своими — ${mine} · Яндексом — ${yandex}`
                                          : hasOwn
                                            ? 'запросов пока не было'
                                            : 'своей реализации нет'}
                                </span>
                            </div>
                            <span class={`pill ${pill.cls}`}>{pill.label}</span>
                        </button>
                    );
                })}
                {rows.length === 0 && (
                    <p class="t-meta m-0 py-12 px-0">
                        Ничего не нашлось.
                    </p>
                )}
            </section>
            <p class="t-caption my-0 mx-6">
                {health?.mode === 'relay' && 'Сейчас режим «Яндекс»: это заработает в режимах «Вместе» и «Своё». '}
                {own && 'Сейчас «Своё»: шаги «Яндекс» пропускаются, а в «Вместе» они снова работают как есть. '}
                {mayEdit ? 'Нажмите на строку, чтобы выбрать, кто отвечает, — сразу, без перезапуска. ' : 'Выбирать, кто отвечает, может администратор. '}
                {/* `.env` и дефолт — слова системы: только в слое разработчика (docs/plans/panel-rebuild.md, «Три слоя»). */}
                {devMode() && (
                    <>
                        ROUTES в .env остаётся дефолтом для новой базы
                        {isDefault === false ? '; стоят изменения из пульта.' : isDefault === true ? '; сейчас стоит дефолт.' : '.'}{' '}
                    </>
                )}
                {rows.length} {plural(rows.length, 'возможность', 'возможности', 'возможностей')}.
            </p>
            {error && (
                <div class="banner-offline my-0 mx-6" role="alert">
                    {error}
                </div>
            )}

            {sheet && (
                <Overlay onDismiss={close}>
                    <div class="sheet glass-strong" role="dialog" aria-modal="true" aria-label={`Кто отвечает: ${sheet.title}`}>
                        <div class="flex items-center gap-8">
                            <h2 class="t-title-xs m-0 flex-1">
                                {sheet.title}
                            </h2>
                            <span class={`pill ${routePill(routes[sheet.name]).cls}`}>{routePill(routes[sheet.name]).label}</span>
                            <button class="pill pill-off pill-btn-sm" onClick={close} aria-label="Закрыть">
                                ✕
                            </button>
                        </div>
                        {OPTIONS.map((option, index) => {
                            const current = routes[sheet.name] === option.value;
                            const marked = pending === option.value || (current && !pending);
                            return (
                                <button
                                    key={option.value}
                                    ref={index === 0 ? firstOption : undefined}
                                    disabled={saving || !mayEdit}
                                    onClick={() => setPending(current ? null : option.value)}
                                    aria-pressed={marked}
                                    class={`flex items-center gap-12 min-h-52 py-0 px-14 r-pill ${marked ? 'bg-icon-bg' : 'bg-none'}${mayEdit ? '' : ' opacity-70'}`}
                                >
                                    <span class={`pill ${option.cls}`}>{option.label}</span>
                                    <span class="t-caption flex-1 text-left">
                                        {option.note}
                                        {own && option.value.includes('yandex') ? ' — в «Своём» без Яндекса' : ''}
                                    </span>
                                </button>
                            );
                        })}
                        {pending && (
                            <div
                                role="status"
                                class="py-12 px-14 r-panel bg-pill-yandex-bg c-pill-yandex-fg"
                            >
                                <p class="m-0 mb-10 fs-13 lh-18 fw-700">
                                    Станет «{routePill(pending).label}» вместо «{routePill(routes[sheet.name]).label}»
                                </p>
                                <div class="flex gap-8">
                                    <button
                                        class="pill pill-own pill-btn"
                                        disabled={saving}
                                        onClick={() => void apply(sheet, pending)}
                                    >
                                        {saving ? 'Применяю…' : 'Применить'}
                                    </button>
                                    <button
                                        class="pill pill-off pill-btn"
                                        disabled={saving}
                                        onClick={() => setPending(null)}
                                    >
                                        Отменить
                                    </button>
                                </div>
                            </div>
                        )}
                        {!mayEdit && (
                            <p class="t-caption m-0">
                                Выбирать, кто отвечает, может администратор.
                            </p>
                        )}
                    </div>
                </Overlay>
            )}
        </>
    );
}
