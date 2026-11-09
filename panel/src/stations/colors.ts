/** Цвета корпуса по id шлюза (gateway/src/stations/looks.rs): как назвать и чем рисовать значок и образец. */
export interface ColorInfo {
    /** «синяя» — в подписи вида: «Лайт 2 с часами, синяя». */
    name: string;
    /** «Синий» — у образца цвета. */
    label: string;
    /** Значок колонки: корпус и крышка. */
    body: string;
    top: string;
}

export const COLORS: Record<string, ColorInfo> = {
    azure: { name: 'синяя', label: 'Синий', body: '#1E9FEF', top: '#7FDFFF' },
    lilac: { name: 'фиолетовая', label: 'Фиолетовый', body: '#8176E7', top: '#C3BDFF' },
    pink: { name: 'розовая', label: 'Розовый', body: '#E659A2', top: '#FFA6D2' },
    green: { name: 'зелёная', label: 'Зелёный', body: '#52B134', top: '#A2E577' },
    coral: { name: 'коралловая', label: 'Коралловый', body: '#F37042', top: '#FFB490' },
    graphite: { name: 'графитовая', label: 'Графит', body: '#30363F', top: '#6E7783' },
    red: { name: 'красная', label: 'Красный', body: '#D5323B', top: '#FF8A8E' },
    orange: { name: 'оранжевая', label: 'Оранжевый', body: '#F07F13', top: '#FFC169' },
    yellow: { name: 'лимонная', label: 'Лимонный', body: '#F2C94C', top: '#FFE9A0' },
    teal: { name: 'бирюзовая', label: 'Бирюзовый', body: '#1FA696', top: '#7DE6D8' },
    sand: { name: 'песочная', label: 'Песочный', body: '#C09A6B', top: '#EBD3AE' },
    copper: { name: 'медная', label: 'Медный', body: '#B26E3F', top: '#E8AE7C' },
    white: { name: 'белая', label: 'Белый', body: '#E7E2D8', top: '#FFFFFF' },
};

export const colorInfo = (id: string | undefined | null): ColorInfo => COLORS[id ?? ''] ?? COLORS.azure;
