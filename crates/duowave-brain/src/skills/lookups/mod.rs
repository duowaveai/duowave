//! Lookups on our side: the weather (Open-Meteo) and the news (RSS or Atom feeds), over HTTP behind a trait.

pub mod http;
pub mod news_catalog;
pub mod news_skill;
pub mod weather_skill;
