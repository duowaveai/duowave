// Кратко о голосе и слухе на странице компьютера («Устройства» → Mac → «Этот компьютер»): две строки со ссылками в
// «Настройки» → «Голос» и «Слух» (screens/voice-home.tsx) вместо двух длинных списков моделей (проверка начисто 19.11).
// Отдельным файлом: страница компьютера стоит и фреймом внутри «Голоса» — без круга импортов.

import { useHelpers } from '../helpers';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';

function Card({ id, title, sub, children }: { id: string; title: string; sub?: string; children: preact.ComponentChildren }) {
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`vb-${id}`}>
            <div class="section-head flex flex-col">
                <h2 id={`vb-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
                {sub && <span class="w-cap">{sub}</span>}
            </div>
            {children}
        </section>
    );
}

/**
 * Страница компьютера («Устройства» → Mac → «Этот компьютер»): вместо двух длинных списков моделей — две строки со
 * ссылками в «Голос» и «Слух», с тем, что сейчас (проверка начисто 19.11: «старые места — кратким видом»).
 */
export function VoiceHearingBrief() {
    const panel = usePanel();
    const { view } = useHelpers();
    const t = tr.voiceHome;
    const own = view?.helpers.find((helper) => helper.builtin);
    const about = (ability: 'voice' | 'hearing'): string | null => {
        const details = own?.abilities.find((entry) => entry.id === ability)?.details;
        return [ability === 'voice' ? details?.voice : null, details?.model].filter(Boolean).join(' · ') || null;
    };
    const rows: Array<{ id: 'voice' | 'hearing'; title: string; sub: string }> = [
        { id: 'voice', title: t.briefVoice, sub: about('voice') ?? t.briefVoiceAbout },
        { id: 'hearing', title: t.briefHearing, sub: about('hearing') ?? t.briefHearingAbout },
    ];
    return (
        <Card id="brief" title={t.briefTitle} sub={t.briefLead}>
            <div class="w-rows">
                {rows.map((row) => (
                    <div key={row.id} class="w-row">
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">{row.title}</span>
                            <span class="w-cap wrap-anywhere">{row.sub}</span>
                        </span>
                        <button class="w-btn" onClick={() => panel.go('settings', row.id)}>
                            {t.briefOpen}
                        </button>
                    </div>
                ))}
            </div>
        </Card>
    );
}
