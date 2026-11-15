//! The program's words for people, by the household's language (`GET /api/v1/apps/host/locale`): Russian and English;
//! a language it does not know — English. `{x}` is replaced by the detail.

const RU: &[(&str, &str)] = &[
    ("not_a_link", "это не ссылка"),
    ("scheme_unknown", "неизвестный вид ссылки"),
    ("scheme_unsupported", "этот протокол приложение не поддерживает"),
    ("bad_link", "ссылка не разбирается"),
    ("no_address", "нет адреса или порта сервера"),
    ("no_user", "нет id пользователя"),
    ("no_password", "нет пароля"),
    ("security_unsupported", "защита «{x}» не поддерживается"),
    ("transport_unsupported", "транспорт «{x}» не поддерживается"),
    ("xhttp_mode", "режим XHTTP «{x}» не поддерживается"),
    ("flow_unsupported", "flow «{x}» не поддерживается"),
    ("vision_needs_tls", "Vision работает только с TLS или Reality"),
    ("reality_no_key", "у Reality нет ключа (pbk)"),
    ("reality_no_sni", "у Reality нет имени сайта (sni)"),
    ("obfs_unsupported", "маскировка «{x}» не поддерживается"),
    ("ss_plugin", "плагин Shadowsocks «{x}» не поддерживается"),
    ("cipher_unsupported", "шифр «{x}» не поддерживается"),
    ("wireguard_keys", "в файле WireGuard нет ключей"),
    ("wireguard_address", "в файле WireGuard нет адреса (Address)"),
    ("wireguard_peers", "в файле WireGuard нужен ровно один сервер [Peer], а их {x}"),
    ("engine_refused", "движок не принял настройки сервера"),
    ("no_connection", "Нет такого подключения."),
    ("no_server", "В подключении нет сервера «{x}»."),
    ("empty_source", "Вставьте ссылку, подписку или файл WireGuard."),
    ("nothing_understood", "Не нашлось ни одной понятной ссылки: {x}."),
    ("not_running", "VPN ещё запускается — попробуйте через несколько секунд."),
    ("server_down", "Сервер не отвечает."),
    ("subscription_failed", "Подписка недоступна: {x}"),
    ("subscription_empty", "В подписке нет ни одной понятной ссылки."),
    ("bad_input", "Непонятный запрос."),
    ("too_many", "Подключений уже {x} — больше не нужно."),
];

const EN: &[(&str, &str)] = &[
    ("not_a_link", "not a link"),
    ("scheme_unknown", "an unknown kind of link"),
    ("scheme_unsupported", "the app does not support this protocol"),
    ("bad_link", "the link does not parse"),
    ("no_address", "no server address or port"),
    ("no_user", "no user id"),
    ("no_password", "no password"),
    ("security_unsupported", "security “{x}” is not supported"),
    ("transport_unsupported", "transport “{x}” is not supported"),
    ("xhttp_mode", "XHTTP mode “{x}” is not supported"),
    ("flow_unsupported", "flow “{x}” is not supported"),
    ("vision_needs_tls", "Vision works only with TLS or Reality"),
    ("reality_no_key", "Reality has no key (pbk)"),
    ("reality_no_sni", "Reality has no site name (sni)"),
    ("obfs_unsupported", "obfuscation “{x}” is not supported"),
    ("ss_plugin", "Shadowsocks plugin “{x}” is not supported"),
    ("cipher_unsupported", "cipher “{x}” is not supported"),
    ("wireguard_keys", "the WireGuard file has no keys"),
    ("wireguard_address", "the WireGuard file has no Address"),
    ("wireguard_peers", "a WireGuard file needs exactly one [Peer], it has {x}"),
    ("engine_refused", "the engine did not accept the server's settings"),
    ("no_connection", "No such connection."),
    ("no_server", "The connection has no server “{x}”."),
    ("empty_source", "Paste a link, a subscription or a WireGuard file."),
    ("nothing_understood", "Not a single link it understands: {x}."),
    ("not_running", "The VPN is still starting — try again in a few seconds."),
    ("server_down", "The server does not answer."),
    ("subscription_failed", "The subscription is unavailable: {x}"),
    ("subscription_empty", "The subscription has no link the app understands."),
    ("bad_input", "A request it does not understand."),
    ("too_many", "There are already {x} connections — that is enough."),
];

pub fn t(locale: &str, key: &str, detail: &str) -> String {
    let table = if locale.starts_with("ru") { RU } else { EN };
    let text = table.iter().chain(EN.iter()).find(|(k, _)| *k == key).map(|(_, text)| *text).unwrap_or(key);
    text.replace("{x}", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_know_every_key() {
        assert_eq!(RU.len(), EN.len());
        for (key, _) in RU {
            assert!(EN.iter().any(|(k, _)| k == key), "{key}");
        }
        assert_eq!(t("ru", "cipher_unsupported", "rc4-md5"), "шифр «rc4-md5» не поддерживается");
        assert_eq!(t("de", "server_down", ""), "The server does not answer.");
        assert_eq!(t("en", "no_such_key", ""), "no_such_key");
    }
}
