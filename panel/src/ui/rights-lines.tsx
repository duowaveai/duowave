// Права черновика словами (docs/plans/voice-creation.md, «Что ему можно»): в мастерской — капсулами (новые янтарём,
// ненужные зачёркнуты), в «Примерке» — строками с точкой исполнителя: что уходит из дома — янтарь (с тем, что туда
// уйдёт), остальное — своё.

import type { DraftNeed, InstallPlan } from '../api';
import { permissionTitle } from '../screens/plugins';

/** Капсулы прав для мастерской: новые — янтарём, ненужные — зачёркнуты, прежние — строкой. */
export function RightsLines({ plan }: { plan: InstallPlan }) {
    const rights = plan.kept.filter((name) => !name.startsWith('net:'));
    const hosts = plan.kept.filter((name) => name.startsWith('net:')).map((name) => name.slice(4));
    const nothing = !plan.added.length && !plan.removed.length && !plan.hostsAdded.length && !plan.hostsRemoved.length && !plan.kept.length;
    return (
        <div class="flex flex-col gap-6">
            <h3 class="t-title-xs m-0">Права</h3>
            {nothing && <span class="t-caption">Особых прав не просит.</span>}
            {(plan.added.length > 0 || plan.hostsAdded.length > 0) && (
                <div class="ws-chips">
                    <span class="t-caption">{plan.first ? 'Просит:' : 'Новые:'}</span>
                    {plan.added.map((name) => (
                        <span key={name} class="pill pill-yandex">
                            {permissionTitle(name)}
                        </span>
                    ))}
                    {plan.hostsAdded.map((host) => (
                        <span key={host} class="pill pill-yandex">
                            сеть: {host}
                        </span>
                    ))}
                </div>
            )}
            {(plan.removed.length > 0 || plan.hostsRemoved.length > 0) && (
                <div class="ws-chips">
                    <span class="t-caption">Больше не нужны:</span>
                    {plan.removed.map((name) => (
                        <span key={name} class="pill pill-off line-through">
                            {permissionTitle(name)}
                        </span>
                    ))}
                    {plan.hostsRemoved.map((host) => (
                        <span key={host} class="pill pill-off line-through">
                            сеть: {host}
                        </span>
                    ))}
                </div>
            )}
            {(rights.length > 0 || hosts.length > 0) && (
                <span class="t-caption">
                    Без изменений: {[...rights.map(permissionTitle), ...hosts.map((host) => `сеть: ${host}`)].join(', ')}.
                </span>
            )}
        </div>
    );
}

/** Что уйдёт на хост: ключи и то, что приложение покажет. */
function goesThere(needs: DraftNeed[]): string {
    const keys = needs.filter((need) => need.secret).map((need) => need.title.toLowerCase());
    return keys.length ? `туда уйдут ${keys.join(', ')} и то, что приложение покажет` : 'туда уйдёт то, что приложение спросит';
}

/** Чем право обернётся для человека: «Слышать две фразы колонки», «Своё хранилище и часть пульта». */
function rightLine(name: string, plan: InstallPlan): { title: string; note: string } {
    switch (name) {
        case 'voice.intents':
            return { title: `Слышать ${plan.phrases.length || ''} ${plan.phrases.length === 1 ? 'фразу' : 'фразы'} колонки`.replace('  ', ' '), note: 'остальное идёт мимо него' };
        case 'kv':
            return { title: 'Своё хранилище', note: 'помнит своё, чужого не видит' };
        case 'llm.tools':
            return { title: 'Помогать «Мозгу»', note: 'модель зовёт его, когда нужно' };
        case 'voice.say':
            return { title: 'Говорить колонкой само', note: 'без вашей просьбы' };
        default:
            return { title: permissionTitle(name), note: '' };
    }
}

/** Строки прав для «Примерки»: хосты — янтарём (уходит из дома), остальное — своё. Новые права одобряете вы. */
export function PermissionLines({ plan, needs }: { plan: InstallPlan; needs: DraftNeed[] }) {
    const rights = [...plan.added, ...plan.kept].filter((name, index, all) => !name.startsWith('net:') && all.indexOf(name) === index);
    const hosts = [...plan.hostsAdded, ...plan.kept.filter((name) => name.startsWith('net:')).map((name) => name.slice(4))];
    if (!rights.length && !hosts.length) return <p class="t-caption m-0">Особых прав не просит: работает у вас дома и никуда не ходит.</p>;
    return (
        <div class="fit-rights">
            {hosts.map((host) => (
                <div key={host} class="fit-right">
                    <span class="fit-dot is-out" aria-hidden="true" />
                    <span>
                        Ходить в интернет
                        <span class="t-caption">
                            <code>{host}</code> — {goesThere(needs)}
                        </span>
                    </span>
                </div>
            ))}
            {rights.map((name) => {
                const line = rightLine(name, plan);
                return (
                    <div key={name} class="fit-right">
                        <span class="fit-dot is-own" aria-hidden="true" />
                        <span>
                            {line.title}
                            {line.note && <span class="t-caption">{line.note}</span>}
                        </span>
                    </div>
                );
            })}
            {plan.removed.length + plan.hostsRemoved.length > 0 && (
                <p class="t-caption m-0">Больше не нужны: {[...plan.removed.map(permissionTitle), ...plan.hostsRemoved.map((host) => `сеть: ${host}`)].join(', ')}.</p>
            )}
        </div>
    );
}
