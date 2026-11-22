//! Flash drives and folders attached to a computer as one device (`device/storage`, docs/plans/turbo-devices.md):
//! what is in «Подключено» now. A drive is a thing at hand — the home's only while its computer is at home (owner
//! 20.11, 6); its files are read through the home's folders (`files/home/Connected/<name>`, files.rs).
use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::bus::provider::{BusError, Handle};
use crate::bus::providers::devices::{Device, Instance};
use crate::bus::providers::files::{Files, ATTACHED};
use crate::bus::Level;

pub struct StorageDevice {
    files: Weak<Files>,
}

impl StorageDevice {
    pub fn new(files: &Arc<Files>) -> StorageDevice {
        StorageDevice { files: Arc::downgrade(files) }
    }
}

impl Device for StorageDevice {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn title(&self) -> String {
        crate::tr!("Флешки и диски")
    }

    fn handles(&self) -> Vec<Handle> {
        vec![Handle::new("drives", Level::See).titled(&crate::tr!("какие флешки и папки подключены"))]
    }

    /// While something is attached: an empty «Подключено» is no device.
    fn available(&self) -> bool {
        self.files.upgrade().is_some_and(|files| !files.places().attached().is_empty())
    }

    fn instances(&self) -> Vec<Instance> {
        let Some(files) = self.files.upgrade() else { return Vec::new() };
        files
            .places()
            .attached()
            .into_iter()
            .map(|attached| Instance { id: attached.name.clone(), title: attached.name })
            .collect()
    }

    fn call(&self, handle: &str, _input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
        let answer = match handle {
            "drives" => Ok(json!(self
                .instances()
                .into_iter()
                .map(|instance| json!({ "id": instance.id, "name": instance.title, "folder": format!("files/home/{ATTACHED}/{}", instance.id) }))
                .collect::<Vec<_>>())),
            other => Err(BusError::NotFound(format!("device/storage: {other}"))),
        };
        Box::pin(async move { answer })
    }
}
