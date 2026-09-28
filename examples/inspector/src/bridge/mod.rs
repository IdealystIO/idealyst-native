//! Everything the Inspector knows about talking to a running app, with no
//! UI in it: discovery (which apps are running), the bridge client (the
//! connection and its refresh loop) and the model (typed replies plus the
//! derivations the screens render). A front end other than this desktop
//! app — the CLI, say — can drive the same three pieces.

pub mod client;
pub mod discovery;
pub mod model;
