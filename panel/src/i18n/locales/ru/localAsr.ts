// Тексты: screens/local-asr.tsx.

export default {
    backendMetal: 'видеокарте Мака',
    backendVulkan: 'видеокарте',
    backendCuda: 'видеокарте NVIDIA',
    backendRocm: 'видеокарте AMD',
    backendCpu: 'процессоре',
    backendThis: 'этом компьютере',

    downloading: (done: number, total: number) => `скачиваю модель · ${done} из ${total} МБ`,
    loadingWhile: (serving: string) => `загружаю новую модель, пока работает ${serving}`,
    loading: 'загружаю модель…',
    readyHere: (where: string, device: string, fallback: string) => `работает здесь, на ${where}${device}${fallback}`,
    gpuFailed: ' — видеокарта отказала',
    failedServing: (serving: string) => `работает ${serving}; новая не встала`,
    failed: 'не работает',
    noModel: 'модель не выбрана',

    titleFailed: 'Слух не встал',
    titleSetup: 'Готовлю слух',
    downloadingNote: 'Скачиваю модель распознавания речи — один раз. Дальше она работает на этом компьютере, без интернета.',
    progressLabel: 'Модель распознавания',
    notStarted: 'Распознавание не запустилось.',
    listening: (serving: string) => `Пока слушает ${serving}.`,
    retrying: 'Пробую…',
    retry: 'Повторить',
};
