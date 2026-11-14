import { tr } from '../i18n';

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

/** Название и подпись берутся из словаря при каждом обращении: язык пульта меняется на лету. */
const color = (id: string, body: string, top: string): ColorInfo => ({
    get name() {
        return tr.stations.colorName[id as keyof typeof tr.stations.colorName];
    },
    get label() {
        return tr.stations.colorLabel[id as keyof typeof tr.stations.colorLabel];
    },
    body,
    top,
});

export const COLORS: Record<string, ColorInfo> = {
    azure: color('azure', '#1E9FEF', '#7FDFFF'),
    lilac: color('lilac', '#8176E7', '#C3BDFF'),
    pink: color('pink', '#E659A2', '#FFA6D2'),
    green: color('green', '#52B134', '#A2E577'),
    coral: color('coral', '#F37042', '#FFB490'),
    graphite: color('graphite', '#30363F', '#6E7783'),
    red: color('red', '#D5323B', '#FF8A8E'),
    orange: color('orange', '#F07F13', '#FFC169'),
    yellow: color('yellow', '#F2C94C', '#FFE9A0'),
    teal: color('teal', '#1FA696', '#7DE6D8'),
    sand: color('sand', '#C09A6B', '#EBD3AE'),
    copper: color('copper', '#B26E3F', '#E8AE7C'),
    white: color('white', '#E7E2D8', '#FFFFFF'),
};

export const colorInfo = (id: string | undefined | null): ColorInfo => COLORS[id ?? ''] ?? COLORS.azure;
