// Первые шаги → «Ваш дом» (docs/plans/home-setup-flow.md, этап 4; холст «Онбординг», доска HomeStatus): последний шаг
// на всех путях — что в доме работает, а что починить, у каждой строки своя кнопка (screens/home-status.tsx). Дальше —
// стол, где та же сводка остаётся карточкой, пока не всё зелёное.

import { tr } from '../../i18n';
import { usePanel } from '../../shell/context';
import { HomeStatusRows, useHomeStatus } from '../home-status';
import { useOnboarding } from './nav';
import '../../ui/onboarding-canvas.css';

/** `centre` — колонка-центр, когда компьютер только что вошёл в дом Турбо; `onDone` — своя кнопка «На стол» (путь Турбо). */
export function StatusStep({ centre, onDone }: { centre?: string; onDone?: () => void }) {
    const t = tr.onboarding;
    const panel = usePanel();
    const onboarding = useOnboarding();
    const { view, error, found, reread } = useHomeStatus(centre);
    const go = (section: string, tab?: string): void => {
        onboarding.leave();
        panel.go(section, tab);
    };
    return (
        <div class="onb-split">
            <section class="onb-col" style={{ maxWidth: '640px', gap: '16px' }}>
                <h1 class="h2">{t.statusTitle}</h1>
                <p class="lead" style={{ fontSize: '15px', lineHeight: 1.55 }}>
                    {t.statusAbout}
                </p>
                <div class="card card-tight" style={{ padding: '6px 18px' }}>
                    {view ? (
                        <HomeStatusRows view={view} found={found} centre={centre} onFixed={reread} go={go} />
                    ) : (
                        <p class="small" role="status">
                            {error ?? t.statusReading}
                        </p>
                    )}
                </div>
                {view?.green && found.length === 0 && (
                    <p class="small" role="status" style={{ margin: 0 }}>
                        {t.statusGreen}
                    </p>
                )}
                {onDone && (
                    <div class="onb-row-between">
                        <span />
                        <button type="button" class="dw-btn dw-btn-lg dw-btn-accent" onClick={onDone}>
                            {t.turboToDesk}
                        </button>
                    </div>
                )}
            </section>
        </div>
    );
}
