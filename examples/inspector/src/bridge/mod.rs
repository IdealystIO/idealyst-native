//! Everything the Inspector knows about talking to its server, with no UI
//! in it: the client (one WebSocket to the Inspector server, speaking
//! `inspector-protocol`) and the model (the typed state the server pushes,
//! plus the derivations the screens render).
//!
//! The Inspector never talks to an app. The server (`idealyst inspect`)
//! discovers apps, holds their robot-bridge connections and pushes each
//! front end a snapshot; the client only renders and sends back what the
//! user did. That's what lets the same front end run in a browser.

pub mod client;
pub mod endpoint;
pub mod model;
