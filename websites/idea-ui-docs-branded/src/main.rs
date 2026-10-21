//! The app's entry point — every platform, one line. `entry!` reads this
//! crate's `[package.metadata.idealyst.app]` and mounts [`app`](idea_ui_docs_branded::app)
//! with the scene extensions idea-ui-docs registers.
idealyst::entry!(idea_ui_docs_branded);
