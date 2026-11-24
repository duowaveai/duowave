//! The pretend stations' way out (src/stations/pretend.rs, docs/plans/pretend-stations.md).

use std::sync::Arc;

/// The entry a pretend gateway's HTTP calls to the other pretend addresses take (src/stations/pretend.rs): one of
/// «Выход в интернет»'s entries, always direct.
pub async fn pretend_entry() {
    let decide: crate::egress::app_entry::Decide = Arc::new(|_, _, _| crate::egress::app_entry::Hop::Direct);
    let failed: crate::egress::app_entry::Failed = Arc::new(|_, host, why| {
        tracing::info!(target: "pretend", "{host} did not answer ({why})");
    });
    match crate::egress::app_entry::start(decide, failed).await {
        Ok(port) => crate::utils::net::set_entry(port),
        Err(error) => tracing::warn!(target: "pretend", "no entry for the pretend addresses: {error}"),
    }
}
