import { useState } from 'preact/hooks';

import { api, problemOf } from '../api';
import { tr } from '../i18n';
import { IconLaptop } from '../icons';

/**
 * Строка на главном экране: macOS не пускает DuoWave в домашнюю сеть (нет разрешения «Локальная сеть»). Дом этого
 * компьютера тогда не объявляется, колонок он не находит, и другие компьютеры его не видят, а сам он об этом молчал
 * (находка 3 проверки шага 11). Разрешить можно только в настройках Mac: строка говорит, где, а кнопка открывает их с
 * окном-подсказкой рядом («Разрешения», docs/plans/permissions.md). С телефона кнопка скажет, что открыть можно только
 * на самом компьютере.
 */
export function LocalNetworkBanner() {
    const [problem, setProblem] = useState<string | null>(null);
    const open = () => {
        setProblem(null);
        api.openPermission('localNetwork', true).catch((error: unknown) => setProblem(problemOf(error)));
    };
    return (
        <section class="card glass setup-banner" role="status" aria-label={tr.ui.localNetworkTitle}>
            <span class="picker-glyph" aria-hidden="true">
                <IconLaptop size={22} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class="fs-14 lh-20">{tr.ui.localNetworkTitle}</b>
                <span class="t-caption">{tr.ui.localNetworkAbout}</span>
                {problem && (
                    <span class="t-caption" role="alert">
                        {problem}
                    </span>
                )}
            </div>
            <button type="button" class="btn-ghost" onClick={open}>
                {tr.permissions.open}
            </button>
        </section>
    );
}
