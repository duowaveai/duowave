// Первые шаги → «Разрешения» (docs/plans/permissions.md): перед поиском колонки, чтобы окна системы появились после
// объяснения, а поиск шёл уже с разрешённой сетью. Шаг есть, только когда есть что разрешать (на Mac — при первом
// запуске всегда: «ещё не спрашивали» macOS иначе не отличить), и только в приложении на этом компьютере.
//
// Mac: «Локальная сеть», потом «Микрофон», по очереди; справа — нарисованное окно macOS, которое сейчас появится, с
// курсором на «Разрешить». Windows: только то, что выключено (брандмауэр, общедоступная сеть, микрофон), по шагам в
// строке. Всё разрешено — шаг идёт дальше сам; «Позже» — всегда.

import { useContext, useEffect } from 'preact/hooks';

import type { PermissionItem } from '../../api';
import { tr } from '../../i18n';
import { type Permissions, PermissionRows, SystemPrompt, needs } from '../../ui/permissions';
import { StepNavContext, useOnboarding } from './nav';
import '../../ui/onboarding-canvas.css';

/** Через сколько шаг идёт дальше сам, когда всё разрешили на нём. */
const ON_AFTER_MS = 1200;

export function AllowStep({ perms, items }: { perms: Permissions; items: PermissionItem[] }) {
    const t = tr.permissions;
    const onboarding = useOnboarding();
    const setNav = useContext(StepNavContext);
    const view = perms.view;
    const mac = view?.os === 'mac';
    const rows = view?.items.filter((row) => items.includes(row.item)) ?? [];
    const allOk = rows.length > 0 && rows.every((row) => !needs(row));
    useEffect(() => setNav(allOk ? null : { label: t.later }), [allOk]);
    useEffect(() => {
        if (!allOk) return;
        const timer = window.setTimeout(() => onboarding.next(), ON_AFTER_MS);
        return () => window.clearTimeout(timer);
    }, [allOk]);
    // Рисунок — окна той строки, что на очереди.
    const current = mac ? rows.find(needs)?.item : undefined;
    const picture = current === 'localNetwork' || current === 'microphone' ? current : null;
    return (
        <div class="onb-split">
            <section class="onb-col perm-step-col" style={{ maxWidth: '560px', gap: '16px' }}>
                <h1 class="h2">{mac ? t.stepTitleMac : t.stepTitleWindows}</h1>
                <p class="lead" style={{ fontSize: '15px', lineHeight: 1.55 }}>
                    {mac ? t.stepAboutMac : t.stepAboutWindows}
                </p>
                <PermissionRows perms={perms} items={items} order={mac} look="onb" />
                {allOk && (
                    <p class="small" role="status" style={{ margin: 0 }}>
                        {t.allDone}
                    </p>
                )}
            </section>
            {picture && (
                <section class="onb-col" style={{ maxWidth: '420px', alignItems: 'center', justifyContent: 'center' }}>
                    <SystemPrompt item={picture} />
                </section>
            )}
        </div>
    );
}
