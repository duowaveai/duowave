// Тексты: screens/house.tsx.
import { plural } from '../../format';

export default {
    pillApproved: 'отвечаем',
    pillPending: 'ждёт разрешения',
    pillBlocked: 'не пускаем',
    blockTitle: (name: string) => `Не пускать «${name}»?`,
    blockText: 'Она перестанет отвечать, пока вы её не разрешите.',
    blockOk: 'Не пускать',
    stations: 'Колонки',
    stationsCount: (n: number) => `${n} ${plural(n, { one: 'колонка', few: 'колонки', many: 'колонок', other: 'колонки' })}`,
    noStations: 'Ни одна колонка ещё не подключалась.',
    addressUnknown: 'адрес неизвестен',
    lastSeen: (address: string, ago: string) => `${address} · была ${ago}`,
    allow: 'разрешить',
    block: 'не пускать',
    feedNoAnswer: (error: string) => `Лента не ответила: ${error}`,
    latestInstalled: 'Стоит последняя версия.',
    installTitle: (version: string) => `Поставить версию ${version}?`,
    installTextStation: 'DuoWave перезапустится, колонка переподключится сама.',
    installTextDevice: 'DuoWave перезапустится, пульт подождёт его сам.',
    installTextFromComputer: 'Её поставит компьютер, который ведёт колонку: снимет копию настроек, попробует новую версию из ОЗУ и перезапустит DuoWave. Колонка переподключится сама.',
    fromComputerStarted: 'Компьютер ставит новую версию на колонку — это около минуты.',
    installOk: 'Поставить',
    updates: 'Обновления',
    version: (version: string) => `версия ${version}`,
    urgent: 'Срочное обновление',
    urgentText: ' — исправляет важное в этой версии.',
    urgentAuto: 'DuoWave поставит его сам в течение часа, можно и сейчас.',
    urgentManual: 'Поставьте его как можно скорее.',
    availableVersion: 'Доступна версия ',
    noFeed: 'Лента выпусков не задана: DuoWave не проверяет обновления.',
    checking: 'Проверяю, нет ли новой версии…',
    noNewVersions: (ago: string) => `Новых версий нет · проверено ${ago}`,
    notChecked: 'Ещё не проверял.',
    installing: 'Ставится. DuoWave перезапустится и вернётся через несколько секунд.',
    trialTrying: (version: string, until: string) =>
        `Версия ${version} пробуется из ОЗУ колонки до ${until}. Упадёт — колонка сама вернётся к прежней.`,
    trialWaiting: (version: string) =>
        `Версия ${version} прошла пробу и записана на место прежней: DuoWave перезапустится на пару секунд, когда колонка затихнет.`,
    trialStuck: (version: string, why: string) => `Версия ${version} работает из ОЗУ, но на диск пока не записалась: ${why}`,
    trialAppsPaused: 'Приложения колонки выключены на время пробы и включатся сами.',
    check: 'проверить',
    installingShort: 'ставится…',
    update: 'обновить',
    // Компьютер ставит колонке только свою версию (owner 20.11): выпуск новее компьютера — сначала на компьютер.
    updateBoth: (version: string) => `Обновить компьютер и колонку до ${version}`,
    installTextComputerFirst:
        'Колонке компьютер ставит только свою версию, а она старше этой. Сначала обновится DuoWave на этом компьютере — он перезапустится, окно откроется заново. Потом компьютер сам поставит новую версию колонке: снимет копию настроек, попробует её из ОЗУ и перезапустит DuoWave. Больше ничего нажимать не нужно.',
    computerFirstStarted: (version: string) => `Обновляется DuoWave на этом компьютере. Когда он запустит ${version}, колонка обновится следом сама.`,
    followsComputer: (version: string) => `Колонка обновится до ${version} следом за компьютером — сама, когда он запустит новую версию.`,
    stationAhead: (station: string, computer: string) =>
        `На колонке DuoWave ${station} — новее, чем на этом компьютере (${computer}). Обновите DuoWave на компьютере.`,
    computerFailed: (error: string) => `Компьютер не обновил колонку: ${error}`,
    installTextOwn: 'DuoWave на этом компьютере перезапустится, окно откроется заново.',
    urgentByComputer: 'Компьютер, который ведёт колонку, поставит его сам, когда будет на связи; можно и сейчас.',
    // «Колонка без компьютера» (owner 20.11): только по кнопке.
    installAlone: 'поставить без компьютера',
    installAloneTitle: (version: string) => `Поставить ${version} без компьютера?`,
    installAloneText:
        'Обычно колонку обновляет компьютер, который её ведёт: он снимает копию настроек и ставит свою версию. Без него колонка сама скачает новую версию из выпуска, проверит подпись и попробует её из ОЗУ — упадёт, колонка вернётся к прежней. Когда компьютер вернётся, обновите DuoWave и на нём.',
};
