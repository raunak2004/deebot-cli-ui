//! Ecovacs cloud login and robot commands, shared by `deebot-cli` and `deebot-ui`.
//!
//! [`session`] handles logging in and the saved session; [`robot`] holds every
//! command the robot understands, returning plain data for callers to display.

mod continents;
pub mod ecovacs;
pub mod map;
pub mod robot;
pub mod session;

/// Replies from the robot are JSON; re-exported so callers use the same version
pub use serde_json;
pub use ecovacs::{AccountCredentials, Client, Config, Credentials, Device};
pub use robot::Robot;
pub use session::Login;
