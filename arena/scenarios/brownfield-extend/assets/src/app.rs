//! `app()` — the mount entry point. Builds the shared [`TaskStore`] and
//! hands it to the [`Dashboard`] tree.

use runtime_core::{ui, Element};

use crate::components::dashboard::Dashboard;
use crate::store::TaskStore;

pub fn app() -> Element {
    let store = TaskStore::new();
    ui! {
        Dashboard(store = store)
    }
}
