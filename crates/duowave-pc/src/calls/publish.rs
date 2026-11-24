//! Telegram by the gateway's bot as a way to publish («Поделиться», capabilities/publish.rs): a video to a channel, a
//! contact of the bot or «Оповещения».

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::TelegramMessenger;
use crate::capabilities::publish::{Kind, Published, Publisher, PublisherCard, Request};

/// Telegram by the gateway's bot: to a channel (@name, the bot is its admin), a contact of the bot or «Оповещения».
pub struct Telegram {
    pub bot: Arc<TelegramMessenger>,
}

impl Publisher for Telegram {
    fn card(&self) -> PublisherCard {
        let ready = self.bot.enabled();
        PublisherCard {
            id: "telegram".into(),
            title: crate::tr!("В Telegram своим ботом"),
            kind: Kind::Direct,
            accepts: vec!["video"],
            ready,
            needs: (!ready)
                .then(|| crate::tr!("бота Telegram нет: подключите его в пульте («Приложения» → «Подключения»)")),
            runs: "pc",
            from: "any",
            url: None,
            about: crate::tr!("ролик до 50 МБ — в канал (@имя, бот — его админ), контакту бота или в «Оповещения»"),
        }
    }

    fn publish(&self, request: Request) -> BoxFuture<'static, Result<Published, String>> {
        let bot = self.bot.clone();
        Box::pin(async move {
            let field =
                |key: &str| request.input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let to = field("to");
            let chat = if to.is_empty() {
                bot.alert_chat()
                    .ok_or_else(|| crate::tr!("куда: укажите канал (@имя) или выберите чат «Оповещения» у бота"))?
            } else if to.starts_with('@') || to.trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) {
                to.clone()
            } else {
                bot.contact(&to)
                    .map(|contact| contact.chat_id)
                    .ok_or_else(|| crate::tr!("у бота нет контакта «{to}»", to = to))?
            };
            let bytes = std::fs::read(&request.file).map_err(|error| error.to_string())?;
            let name = request
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "video.mp4".into());
            let caption = field("caption");
            bot.send_video(&chat, bytes, &name, Some(caption.as_str()))
                .await
                .map_err(|error| format!("Telegram: {error}"))?;
            Ok(Published {
                answer: json!({ "sent": true, "to": if to.is_empty() { "alerts".to_string() } else { to.clone() } }),
                line: crate::tr!(
                    "опубликовано: {path} → Telegram {to}",
                    path = request.path,
                    to = if to.is_empty() { crate::tr!("«Оповещения»") } else { to.clone() }
                ),
            })
        })
    }
}
