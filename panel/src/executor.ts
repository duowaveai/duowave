import { PanelMode } from './api';

/**
 * Кто ответит на возможность при этом режиме. В «Шлюзе» решает первый шаг маршрута; в «Своём» шаги к Яндексу
 * пропускаются — маршрут остаётся предпочтением семьи, но Яндекс запрещён, и без нашего шага возможность молчит;
 * в «Яндексе» отвечает Яндекс, что бы ни говорил маршрут.
 */
export function executorOf(route: string | undefined, mode: PanelMode): 'own' | 'yandex' | 'off' {
    if (mode === 'relay') return 'yandex';
    const steps = stepsOf(route);
    return (mode === 'own' ? steps.find((step) => step === 'own') : steps[0]) ?? 'off';
}

/** Маршрут с другим первым шагом: остальные шаги остаются запасными («own,yandex» → «yandex,own»). */
export function withFirst(route: string | undefined, first: 'own' | 'yandex'): string {
    return [first, ...stepsOf(route).filter((step) => step !== first)].join(',');
}

function stepsOf(route: string | undefined): Array<'own' | 'yandex'> {
    return (route ?? '').split(',').filter((step): step is 'own' | 'yandex' => step === 'own' || step === 'yandex');
}
