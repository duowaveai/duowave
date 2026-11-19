// Тексты: screens/auth.tsx — первая настройка и вход.

export default {
    passwordShort: 'Пароль — хотя бы 8 символов.',
    passwordsDiffer: 'Пароли не совпадают.',
    codeNeeded: 'Введите код настройки: его показал установщик или журнал DuoWave.',
    nameNeeded: 'Введите имя.',
    welcome: 'Добро пожаловать в DuoWave',
    panel: 'Пульт',
    setupSteps: 'Сначала пароль администратора: первый, кто его задаёт, управляет пультом. Дальше — где будет жить DuoWave и что он умеет.',
    loginNamed: 'Имя и пароль — как администратор вас записал.',
    loginPassword: 'Введите пароль администратора.',
    setupCode: 'Код настройки',
    setupCodeHint: 'Строка «первая настройка пульта» в журнале DuoWave: на Mac — logs/duowave.log, в Docker — docker compose logs gateway.',
    setupCodeHintStation: (address: string) =>
        `Его показал установщик в конце установки на колонку. Потерян — в Терминале компьютера: ssh -p 2222 root@${address} "grep 'первая настройка' /data/opt/duowave/gateway/gateway.log | tail -n 1"`,
    name: 'Имя',
    adminPassword: 'Пароль администратора',
    password: 'Пароль',
    passwordPlaceholder: 'от 8 символов',
    passwordAgain: 'Пароль ещё раз',
    wait: 'Секунду…',
    saveAndEnter: 'Сохранить и войти',
    enter: 'Войти',
    setupNoteCode: 'Вы настраиваете не с того компьютера, где стоит DuoWave, поэтому нужен код. Пароль хранится в базе DuoWave в виде хэша.',
    setupNote: 'Пароль хранится в базе DuoWave в виде хэша. Людей и их права добавите потом: «Настройки» → «Люди и доступ».',
    loginNote: 'Пульт запомнит это устройство на месяц.',
};
