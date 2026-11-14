import { useEffect, useRef, useState } from 'preact/hooks';
import { devMode } from '../shell/dev';

import { AccessCatalog, Health, RoutesSetting, api, can, problemOf } from '../api';
import { CAPABILITIES, CapabilityMeta, capabilityIcon } from '../capabilities';
import { capabilityIconComponent } from '../icons';
import { Overlay } from '../ui/sheet';
import { tr } from '../i18n';

function routePill(route: string | undefined): { cls: string; label: string } {
    const t = tr.routes;
    switch (route) {
        case 'own':
            return { cls: 'pill-own', label: t.pillOwn };
        case 'own,yandex':
            return { cls: 'pill-own', label: t.pillOwnYandex };
        case 'yandex':
            return { cls: 'pill-yandex', label: t.pillYandex };
        case 'yandex,own':
            return { cls: 'pill-yandex', label: t.pillYandexOwn };
        default:
            return { cls: 'pill-off', label: route === 'off' ? t.pillOff : '—' };
    }
}

/** Что можно выбрать в шите: значения те же, что понимает ROUTES. */
const options = (): Array<{ value: string; label: string; cls: string; note: string }> => [
    { value: 'own', label: tr.routes.pillOwn, cls: 'pill-own', note: tr.routes.noteOwn },
    { value: 'own,yandex', label: tr.routes.pillOwnYandex, cls: 'pill-own', note: tr.routes.noteOwnYandex },
    { value: 'yandex', label: tr.routes.pillYandex, cls: 'pill-yandex', note: tr.routes.noteYandex },
    { value: 'off', label: tr.routes.pillOff, cls: 'pill-off', note: tr.routes.noteOff },
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

export function RoutesScreen({ health, catalog, title = tr.routes.whoAnswers }: Props) {
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
                    {tr.routes.waitingGateway}
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
                        placeholder={tr.routes.search}
                        aria-label={tr.routes.searchLabel}
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
                            aria-label={`${capability.title}: ${pill.label}${mayEdit ? tr.routes.changeSuffix : ''}`}
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
                                          ? tr.routes.served(mine, yandex)
                                          : hasOwn
                                            ? tr.routes.noRequests
                                            : tr.routes.noOwnImplementation}
                                </span>
                            </div>
                            <span class={`pill ${pill.cls}`}>{pill.label}</span>
                        </button>
                    );
                })}
                {rows.length === 0 && (
                    <p class="t-meta m-0 py-12 px-0">
                        {tr.routes.nothingFound}
                    </p>
                )}
            </section>
            <p class="t-caption my-0 mx-6">
                {health?.mode === 'relay' && tr.routes.relayNote}
                {own && tr.routes.ownNote}
                {mayEdit ? tr.routes.mayEditNote : tr.routes.adminOnlyNote}
                {/* `.env` и дефолт — слова системы: только в слое разработчика (docs/plans/panel-rebuild.md, «Три слоя»). */}
                {devMode() && (
                    <>
                        {tr.routes.envDefault}
                        {isDefault === false ? tr.routes.envChanged : isDefault === true ? tr.routes.envIsDefault : '.'}{' '}
                    </>
                )}
                {tr.routes.count(rows.length)}
            </p>
            {error && (
                <div class="banner-offline my-0 mx-6" role="alert">
                    {error}
                </div>
            )}

            {sheet && (
                <Overlay onDismiss={close}>
                    <div class="sheet glass-strong" role="dialog" aria-modal="true" aria-label={tr.routes.sheetLabel(sheet.title)}>
                        <div class="flex items-center gap-8">
                            <h2 class="t-title-xs m-0 flex-1">
                                {sheet.title}
                            </h2>
                            <span class={`pill ${routePill(routes[sheet.name]).cls}`}>{routePill(routes[sheet.name]).label}</span>
                            <button class="pill pill-off pill-btn-sm" onClick={close} aria-label={tr.routes.close}>
                                ✕
                            </button>
                        </div>
                        {options().map((option, index) => {
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
                                        {own && option.value.includes('yandex') ? tr.routes.withoutYandexInOwn : ''}
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
                                    {tr.routes.willBecome(routePill(pending).label, routePill(routes[sheet.name]).label)}
                                </p>
                                <div class="flex gap-8">
                                    <button
                                        class="pill pill-own pill-btn"
                                        disabled={saving}
                                        onClick={() => void apply(sheet, pending)}
                                    >
                                        {saving ? tr.routes.applying : tr.routes.apply}
                                    </button>
                                    <button
                                        class="pill pill-off pill-btn"
                                        disabled={saving}
                                        onClick={() => setPending(null)}
                                    >
                                        {tr.routes.cancel}
                                    </button>
                                </div>
                            </div>
                        )}
                        {!mayEdit && (
                            <p class="t-caption m-0">
                                {tr.routes.adminOnly}
                            </p>
                        )}
                    </div>
                </Overlay>
            )}
        </>
    );
}
