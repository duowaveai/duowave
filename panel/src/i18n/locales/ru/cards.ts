// Тексты: cards/*.
import { plural } from '../../format';

export default {
    lineLabel: (from: string, to: string, min: string, max: string) => `Линия: от ${from} до ${to}, наименьшее ${min}, наибольшее ${max}`,
    barsLabel: (values: string) => `Столбики: ${values}`,
    ready: 'готово',
    pictureMissing: (name?: string) => `Картинка не найдена${name ? `: ${name}` : ''}`,
    fieldLabel: 'Поле карточки',
    send: 'Отправить',
    nothingYet: 'Пока показывать нечего.',
    notACard: (fn: string) => `Функция ${fn} вернула не карточку.`,
    loading: 'Загружаю…',
    fnFailed: (fn: string) => `${fn} не отработала`,
    loadingCard: 'Загружаю карточку…',
};
