//! «Связи» upstairs (docs/plans/bus.md): the cards apps show, the panel's routes, and the providers — apps, devices,
//! the system, the home folder — each beside what it serves. The core — the addresses, the ladder, the rule, the
//! trail, a person's «да», the journal, the provider's contract, the links kept — is the floor «Права» (src/rights);
//! its names are here as well, so `bus::Bus`, `bus::Trail` and `bus::address` read as before.

pub mod cards;
pub mod providers;
pub mod routes;

use std::sync::Arc;

pub use crate::rights::*;

#[cfg(test)]
pub(crate) mod tests;

crate::route_needs!(Arc<Bus>, Arc<crate::plugins::PluginHost>);

/// The folders' provider as the bus keeps it (`Bus::keep_folders`): set at the start and typed again here for the
/// gateway's own work on the folders (the start's folders, the hourly sweep).
pub trait BusFolders {
    fn set_files(&self, files: Arc<providers::files::Files>);
    fn files(&self) -> Option<Arc<providers::files::Files>>;
}

impl BusFolders for Bus {
    fn set_files(&self, files: Arc<providers::files::Files>) {
        self.keep_folders(files);
    }

    fn files(&self) -> Option<Arc<providers::files::Files>> {
        self.kept_folders()?.downcast::<providers::files::Files>().ok()
    }
}
