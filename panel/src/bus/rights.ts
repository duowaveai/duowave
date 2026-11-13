// Права приложения словами — одна функция на установку, окно приложения и мастерскую (docs/plans/bus.md, «Один
// способ»). С Host API 2 прав два вида: куда приложение ходит в интернет (`net:<хост>`) и до чего в доме дотягивается
// по шине (`see:/listen:/call:/edit:<адрес>`). Связи спрашиваются «да» при первом обращении, как в телефоне.

/** Участники, которых люди знают по имени. */
const KNOWN: Record<string, { title: string; note: string }> = {
    'system/voice': { title: 'Говорить колонкой само', note: 'без вашей просьбы' },
    'system/notify': { title: 'Писать вам', note: 'в Telegram или уведомлением' },
    'system/schedule': { title: 'Действовать по расписанию', note: 'само, в своё время' },
    'system/smart-home': { title: 'Умный дом', note: 'видеть и включать устройства' },
    'system/content': { title: 'Ваша музыка и сказки', note: 'находить и включать' },
    'system/home': { title: 'Город погоды и ленты новостей', note: 'то, что выбрано в пульте' },
    'system/brain': { title: 'Отвечать моделью «Мозга»', note: 'вашими словами в его просьбе' },
    'device/midi': { title: 'MIDI-клавиатура', note: 'слышать и играть её, пока окно открыто' },
    'app/*': { title: 'Звать другие приложения', note: 'их инструменты и функции' },
};

const VERBS: Record<string, string> = { see: 'Видеть', listen: 'Слышать', call: 'Звать', edit: 'Править' };
const FILE_VERBS: Record<string, string> = { see: 'Видеть папку', listen: 'Слышать новости папки', call: 'Читать папку', edit: 'Менять файлы в папке' };

/** Право — для людей: что оно даёт, пояснение и уходит ли что-то из дома. */
export function rightWords(name: string): { title: string; note: string; out: boolean } {
    if (name.startsWith('net:')) return { title: `Ходить в интернет: ${name.slice(4)}`, note: '', out: true };
    const bus = /^(see|listen|call|edit):(.+)$/.exec(name);
    if (!bus) return { title: name, note: '', out: false };
    const [, level, pattern] = bus;
    const known = KNOWN[pattern];
    if (known) return { ...known, out: false };
    if (pattern.startsWith('files/')) {
        const folder = pattern.replace(/\/\*$/, '').split('/').pop() ?? pattern;
        return { title: `${FILE_VERBS[level] ?? level} «${folder}»`, note: '', out: false };
    }
    if (pattern.startsWith('app/')) {
        const id = pattern.slice(4).replace(/\/\*$/, '');
        return { title: `${VERBS[level] ?? level} приложение ${id}`, note: '', out: false };
    }
    return { title: `${VERBS[level] ?? level} ${pattern}`, note: '', out: false };
}

/** Право одной строкой: «Умный дом», «Ходить в интернет: api.example.com». */
export function rightTitle(name: string): string {
    return rightWords(name).title;
}
