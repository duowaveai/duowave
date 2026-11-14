// Права черновика словами (docs/plans/voice-creation.md, «Что ему можно»): в мастерской — капсулами (новые янтарём,
// ненужные зачёркнуты), в «Примерке» — строками с точкой исполнителя: что уходит из дома — янтарь (с тем, что туда
// уйдёт), остальное — своё.

import type { DraftNeed, InstallPlan } from '../api';
import { rightTitle as permissionTitle, rightWords } from '../bus/rights';
import { tr } from '../i18n';

/** Капсулы прав для мастерской: новые — янтарём, ненужные — зачёркнуты, прежние — строкой. */
export function RightsLines({ plan }: { plan: InstallPlan }) {
    const rights = plan.kept.filter((name) => !name.startsWith('net:'));
    const hosts = plan.kept.filter((name) => name.startsWith('net:')).map((name) => name.slice(4));
    const nothing = !plan.added.length && !plan.removed.length && !plan.hostsAdded.length && !plan.hostsRemoved.length && !plan.kept.length;
    return (
        <div class="flex flex-col gap-6">
            <h3 class="t-title-xs m-0">{tr.ui.rightsTitle}</h3>
            {nothing && <span class="t-caption">{tr.ui.rightsNone}</span>}
            {(plan.added.length > 0 || plan.hostsAdded.length > 0) && (
                <div class="ws-chips">
                    <span class="t-caption">{plan.first ? tr.ui.rightsAsks : tr.ui.rightsNew}</span>
                    {plan.added.map((name) => (
                        <span key={name} class="pill pill-yandex">
                            {permissionTitle(name)}
                        </span>
                    ))}
                    {plan.hostsAdded.map((host) => (
                        <span key={host} class="pill pill-yandex">
                            {tr.ui.netHost(host)}
                        </span>
                    ))}
                </div>
            )}
            {(plan.removed.length > 0 || plan.hostsRemoved.length > 0) && (
                <div class="ws-chips">
                    <span class="t-caption">{tr.ui.rightsNotNeeded}</span>
                    {plan.removed.map((name) => (
                        <span key={name} class="pill pill-off line-through">
                            {permissionTitle(name)}
                        </span>
                    ))}
                    {plan.hostsRemoved.map((host) => (
                        <span key={host} class="pill pill-off line-through">
                            {tr.ui.netHost(host)}
                        </span>
                    ))}
                </div>
            )}
            {(rights.length > 0 || hosts.length > 0) && (
                <span class="t-caption">
                    {tr.ui.rightsUnchanged([...rights.map(permissionTitle), ...hosts.map((host) => tr.ui.netHost(host))].join(', '))}
                </span>
            )}
        </div>
    );
}

/** Что уйдёт на хост: ключи и то, что приложение покажет. */
function goesThere(needs: DraftNeed[]): string {
    const keys = needs.filter((need) => need.secret).map((need) => need.title.toLowerCase());
    return keys.length ? tr.ui.goesKeys(keys.join(', ')) : tr.ui.goesAsked;
}

/** Чем право обернётся для человека: «Умный дом — видеть и включать устройства». Связь спросит «да» при первом
 *  обращении, как в телефоне. */
function rightLine(name: string): { title: string; note: string } {
    const words = rightWords(name);
    return { title: words.title, note: [words.note, tr.ui.asksYesFirst].filter(Boolean).join(' · ') };
}

/** Строки прав для «Примерки»: хосты — янтарём (уходит из дома), остальное — своё. Новые права одобряете вы. */
export function PermissionLines({ plan, needs }: { plan: InstallPlan; needs: DraftNeed[] }) {
    const rights = [...plan.added, ...plan.kept].filter((name, index, all) => !name.startsWith('net:') && all.indexOf(name) === index);
    const hosts = [...plan.hostsAdded, ...plan.kept.filter((name) => name.startsWith('net:')).map((name) => name.slice(4))];
    if (!rights.length && !hosts.length) return <p class="t-caption m-0">{tr.ui.rightsNoneHome}</p>;
    return (
        <div class="fit-rights">
            {hosts.map((host) => (
                <div key={host} class="fit-right">
                    <span class="fit-dot is-out" aria-hidden="true" />
                    <span>
                        {tr.ui.goOnline}
                        <span class="t-caption">
                            <code>{host}</code> — {goesThere(needs)}
                        </span>
                    </span>
                </div>
            ))}
            {rights.map((name) => {
                const line = rightLine(name);
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
                <p class="t-caption m-0">{tr.ui.rightsNotNeededList([...plan.removed.map(permissionTitle), ...plan.hostsRemoved.map((host) => tr.ui.netHost(host))].join(', '))}</p>
            )}
        </div>
    );
}
