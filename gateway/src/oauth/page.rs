//! The page a service's return lands on in the owner's browser: words for people and the way back to the panel. The
//! sign-ins of the model providers (src/oauth) and of the MCP servers (src/mcp) answer with it.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

pub fn page(status: StatusCode, text: &str) -> Response {
    let page = format!(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width, initial-scale=1\"><title>Вход</title></head>\
         <body style=\"font-family: system-ui, sans-serif; background: #090C10; color: #F3F6F8; display: grid; \
         place-items: center; min-height: 100vh; margin: 0; padding: 16px;\">\
         <p style=\"max-width: 420px; line-height: 1.5;\">{} <a href=\"/panel/\" style=\"color: #B6F36A;\">Вернуться в \
         пульт</a></p></body></html>",
        escape_html(text)
    );
    (status, Html(page)).into_response()
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_the_page_text() {
        assert_eq!(escape_html(r#"<a href="x">'&'</a>"#), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
    }
}
