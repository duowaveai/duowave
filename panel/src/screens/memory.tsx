// Память и место плагинов (docs/plans/plugins.md, «Память и место на колонке»). «Настройки» → «Колонка» → «Ресурсы
// колонки» (так названа с 12.11, чтобы «память» в пульте значила только память персонажей, screens/recall.tsx) —
// только когда шлюз на колонке: там памяти мало и свопа нет. Полоса — система и Алиса, сам шлюз, плагины, свободно; под
// ней — сколько занято сейчас и сколько ещё можно загрузить, и давление памяти. Бюджет занимают только загруженные
// плагины; включённый, но не загруженный — ничего. Плагин в списке «Пакетов» показывает свою долю — `pluginMemoryLine`.

import { useEffect, useState } from 'preact/hooks';

import { PackageView, PackagesResources, PluginFilesSummary, api, can, problemOf, sessionStore } from '../api';
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

const PRESSURE: Record<PackagesResources['pressure'], { text: string; cls: string }> = {
    normal: { text: 'давление в норме', cls: 'pill-own' },
    warn: { text: 'памяти мало', cls: 'pill-yandex' },
    critical: { text: 'памяти очень мало', cls: 'pill-error' },
};

/** Сегменты полосы: система и Алиса, шлюз, плагины, свободно — в мегабайтах. */
export function memorySegments(r: PackagesResources): Array<{ id: string; label: string; mb: number }> {
    const gateway = Math.max(0, (r.gatewayMb ?? 0) - r.pluginsMb);
    const system = Math.max(0, r.totalMb - r.availableMb - gateway - r.pluginsMb);
    return [
        { id: 'system', label: 'Система и Алиса', mb: system },
        { id: 'gateway', label: 'DuoWave', mb: gateway },
        { id: 'plugins', label: 'Приложения', mb: r.pluginsMb },
        { id: 'free', label: 'Свободно', mb: r.availableMb },
    ];
}

/** «Дом» → «Шлюз»: память колонки. На компьютере её не показываем — там памяти хватает. */
export function StationMemoryCard() {
    const { resources, error } = useResources();
    if (error && !resources) return null;
    if (!resources || !resources.enforced) return null;
    const total = Math.max(1, resources.totalMb);
    const segments = memorySegments(resources);
    const pressure = PRESSURE[resources.pressure];
    return (
        <section class="card glass card-list" aria-labelledby="memory-h">
            <div class="section-head">
                <h2 id="memory-h" class="t-title-xs m-0">
                    Ресурсы колонки
                </h2>
                {resources.measured && <span class={`pill ${pressure.cls}`}>{pressure.text}</span>}
            </div>
            {resources.measured ? (
                <>
                    <div class="mem-bar" role="img" aria-label={segments.map((s) => `${s.label}: ${s.mb} МБ`).join(', ')}>
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
                                {segment.label} · {segment.mb} МБ
                            </li>
                        ))}
                    </ul>
                    {/* Шлюз пишет сам: «Загружено приложений: 2, под них занято 8 МБ. Можно загрузить ещё 70 МБ.» */}
                    <p class="t-body-sm m-0">{resources.note}</p>
                    <p class="t-caption m-0">
                        Бюджет приложений — {resources.budgetMb} МБ. Приложение занимает в нём свой объём, только пока загружено; простаивающие выгружаются и
                        уступают место. Запас для системы и Алисы — {resources.reserveMb} МБ.
                        {resources.action === 'warn'
                            ? ' Под сильным давлением приложения не выключаются — только предупреждение.'
                            : ' Под сильным давлением последнее включённое из загруженных приложений выключится само.'}
                    </p>
                    {resources.disk.freeMb != null && (
                        <p class="t-caption m-0">
                            Место: свободно {resources.disk.freeMb} МБ, приложения занимают {resources.disk.packagesMb} МБ, {resources.disk.reserveMb} МБ — запас на обновление шлюза.
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
    const mb = (bytes: number) => (bytes / (1024 * 1024)).toFixed(1).replace('.', ',');
    return (
        <p class="t-caption m-0">
            Картинки приложений: {mb(summary.bytes)} МБ{summary.limitBytes ? ` из ${mb(summary.limitBytes)}` : ''}.
        </p>
    );
}

/** Доля плагина одной строкой: «память 0,4 из 4 МБ · вызовов 12». */
export function pluginMemoryLine(entry: PackageView): string | null {
    const kind = entry.kind ?? 'plugin';
    if ((kind !== 'plugin' && kind !== 'app') || !entry.budget) return null;
    const used = entry.memory ? (entry.memory.allocated / 1_048_576).toFixed(1).replace('.', ',') : null;
    const parts = [used ? `память ${used} из ${entry.budget.memoryMb} МБ` : `бюджет ${entry.budget.memoryMb} МБ`];
    if (entry.calls > 0) parts.push(`вызовов ${entry.calls}`);
    if (entry.errors > 0) parts.push(`ошибок ${entry.errors}`);
    return parts.join(' · ');
}
