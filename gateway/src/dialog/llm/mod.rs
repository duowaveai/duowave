//! The models: the client interface and its providers, and the brain that says which one answers.

pub mod anthropic;
pub mod brain;
pub mod echo;
pub mod llm_client;
pub mod openai_chat;
pub mod openai_responses;
pub mod sse;

pub use llm_client::*;

/// A request's failure with its causes, for the log and the panel's reasons: «error sending request for url (…):
/// client error (Connect): tcp connect error: Connection refused».
pub(crate) fn describe_http_error(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
