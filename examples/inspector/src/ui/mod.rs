//! The Inspector's screens, built from idea-ui. They only render the
//! [`Snapshot`](crate::bridge::model::Snapshot) the client produces and
//! send action verbs back through [`crate::action`].

pub mod components;
pub mod connect;
pub mod logs;
pub mod navigation;
pub mod shell;
pub mod signals;
pub mod styles;
pub mod theme_toggle;

#[cfg(test)]
mod tests;
