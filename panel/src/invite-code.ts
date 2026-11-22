// Код приглашения DuoWave — каким его ждёт кабинет: `s_<ключ>`, как в ссылке-приглашении
// `https://my.duowave.app/?start=s_<ключ>` (кабинет ищет кампанию по значению `start` целиком; план кабинета —
// bonsaai-appfather, projects/llm-gateway/PLAN.md). Человек может вписать код без `s_`, с заглавной `S_` или вставить всю
// ссылку — пульт приводит к одному виду, прежде чем отдать шлюзу (PUT /api/v1/brain/invite; шлюз кладёт код во вход как
// есть).
// Без DOM и без шлюза: проверяется в scripts/panel-check.ts.

const PREFIX = 's_';

/**
 * Код из того, что вписали: из ссылки — её `start`, `S_` — как `s_`, без `s_` — с ним; сам ключ после `s_` — как
 * вписан. Пусто — пустая строка (забыть код).
 */
export function inviteCode(text: string): string {
    let code = text.trim();
    if (!code) return '';
    // Ссылка-приглашение, с адресом или без: берём её `start`.
    const start = /[?&]start=([^&#\s]*)/i.exec(code);
    if (start) {
        try {
            code = decodeURIComponent(start[1].replace(/\+/g, ' ')).trim();
        } catch {
            code = start[1].trim();
        }
        if (!code) return '';
    }
    return /^s_/i.test(code) ? `${PREFIX}${code.slice(PREFIX.length)}` : `${PREFIX}${code}`;
}
