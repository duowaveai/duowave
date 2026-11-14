// Память и место плагинов (docs/plans/plugins.md, «Память и место на колонке»). «Настройки» → «Колонка» → «Ресурсы
// колонки» (так названа с 12.11, чтобы «память» в пульте значила только память персонажей, screens/recall.tsx) —
// только когда шлюз на колонке: там памяти мало и свопа нет. Полоса — система и Алиса, сам шлюз, плагины, свободно; под
// ней — сколько занято сейчас и сколько ещё можно загрузить, и давление памяти. Бюджет занимают только загруженные
// плагины; включённый, но не загруженный — ничего. Плагин в списке «Пакетов» показывает свою долю — `pluginMemoryLine`.

import { useEffect, useState } from 'preact/hooks';

import { PackageView, PackagesResources, PluginFilesSummary, api, can, problemOf, sessionStore } from '../api';
import { formatNumber, tr } from '../i18n';
import { usePanel } from '../shell/context';
import { HistoryRoomLine } from './history';

/** История приложений и её место — администратору (её API — его). */
const isAdminHere = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Память и место плагинов; перечитывается, когда шлюз говорит о переменах (plugins.resources, packages.changed). */
export function useResources(): { resources: PackagesResources | null; error: string | null } {
    const panel = usePanel();
    const [resources, setResources] = useState<PackagesResources | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        if (!can('connectors.view')) return;
        let alive = true;
        api.packagesResources()
            .then((answer) => {
                if (!alive) return;
                setResources(answer);
                setError(null);
            })
            .catch((e) => alive && setError(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [panel.version]);
    return { resources, error };
}

const pressureOf = (): Record<PackagesResources['pressure'], { text: string; cls: string }> => ({
    normal: { text: tr.memory.pressureNormal, cls: 'pill-own' },
    warn: { text: tr.memory.pressureWarn, cls: 'pill-yandex' },
    critical: { text: tr.memory.pressureCritical, cls: 'pill-error' },
});

/** Сегменты полосы: система и Алиса, шлюз, плагины, свободно — в мегабайтах. */
export function memorySegments(r: PackagesResources): Array<{ id: string; label: string; mb: number }> {
    const gateway = Math.max(0, (r.gatewayMb ?? 0) - r.pluginsMb);
    const system = Math.max(0, r.totalMb - r.availableMb - gateway - r.pluginsMb);
    return [
        { id: 'system', label: tr.memory.segSystem, mb: system },
        { id: 'gateway', label: tr.memory.segGateway, mb: gateway },
        { id: 'plugins', label: tr.memory.segPlugins, mb: r.pluginsMb },
        { id: 'free', label: tr.memory.segFree, mb: r.availableMb },
    ];
}

/** «Дом» → «Шлюз»: память колонки. На компьютере её не показываем — там памяти хватает. */
export function StationMemoryCard() {
    const { resources, error } = useResources();
    if (error && !resources) return null;
    if (!resources || !resources.enforced) return null;
    const total = Math.max(1, resources.totalMb);
    const segments = memorySegments(resources);
    const pressure = pressureOf()[resources.pressure];
    return (
        <section class="card glass card-list" aria-labelledby="memory-h">
            <div class="section-head">
                <h2 id="memory-h" class="t-title-xs m-0">
                    {tr.memory.title}
                </h2>
                {resources.measured && <span class={`pill ${pressure.cls}`}>{pressure.text}</span>}
            </div>
            {resources.measured ? (
                <>
                    <div class="mem-bar" role="img" aria-label={segments.map((s) => `${s.label}: ${tr.memory.mb(s.mb)}`).join(', ')}>
                        {segments
                            .filter((segment) => segment.mb > 0)
                            .map((segment) => (
                                <span key={segment.id} class={`mem-seg mem-${segment.id}`} style={{ width: `${(segment.mb / total) * 100}%` }} />
                            ))}
                    </div>
                    <ul class="mem-legend">
                        {segments.map((segment) => (
                            <li key={segment.id}>
                                <span class={`mem-dot mem-${segment.id}`} />
                                {segment.label} · {tr.memory.mb(segment.mb)}
                            </li>
                        ))}
                    </ul>
                    {/* Шлюз пишет сам: «Загружено приложений: 2, под них занято 8 МБ. Можно загрузить ещё 70 МБ.» */}
                    <p class="t-body-sm m-0">{resources.note}</p>
                    <p class="t-caption m-0">
                        {tr.memory.budget(resources.budgetMb, resources.reserveMb)}
                        {resources.action === 'warn' ? tr.memory.actionWarn : tr.memory.actionOff}
                    </p>
                    {resources.disk.freeMb != null && (
                        <p class="t-caption m-0">
                            {tr.memory.disk(resources.disk.freeMb, resources.disk.packagesMb, resources.disk.reserveMb)}
                        </p>
                    )}
                    <PicturesLine />
                    {isAdminHere() && <HistoryRoomLine />}
                </>
            ) : (
                <p class="t-caption m-0">{resources.note}</p>
            )}
        </section>
    );
}

/** Картинки плагинов на колонке (этап 10): сколько их вместе и из скольких можно. Нет картинок — строки нет. */
function PicturesLine() {
    const panel = usePanel();
    const [summary, setSummary] = useState<PluginFilesSummary | null>(null);
    useEffect(() => {
        api.pluginFilesSummary()
            .then(setSummary)
            .catch(() => setSummary(null));
    }, [panel.version]);
    if (!summary || summary.bytes === 0) return null;
    const mb = (bytes: number) => formatNumber(bytes / (1024 * 1024), { minimumFractionDigits: 1, maximumFractionDigits: 1 });
    return (
        <p class="t-caption m-0">
            {tr.memory.pictures(mb(summary.bytes), summary.limitBytes ? mb(summary.limitBytes) : null)}
        </p>
    );
}

/** Доля плагина одной строкой: «память 0,4 из 4 МБ · вызовов 12». */
export function pluginMemoryLine(entry: PackageView): string | null {
    const kind = entry.kind ?? 'plugin';
    if ((kind !== 'plugin' && kind !== 'app') || !entry.budget) return null;
    const used = entry.memory ? formatNumber(entry.memory.allocated / 1_048_576, { minimumFractionDigits: 1, maximumFractionDigits: 1 }) : null;
    const parts = [used ? tr.memory.lineMemory(used, entry.budget.memoryMb) : tr.memory.lineBudget(entry.budget.memoryMb)];
    if (entry.calls > 0) parts.push(tr.memory.lineCalls(entry.calls));
    if (entry.errors > 0) parts.push(tr.memory.lineErrors(entry.errors));
    return parts.join(' · ');
}
