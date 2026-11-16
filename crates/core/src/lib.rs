//! Account ledger core: one isolated home per account, secrets in the OS keychain,
//! and the harness CLI as the only owner of its own OAuth tokens.

pub mod kit;
pub mod launch;
pub mod limits;
pub mod model;
pub mod secrets;
pub mod store;

pub use anyhow::Result;

pub use model::{Account, Auth, Config, Harness, Kit, Provider, Tool};

pub use store::Store;
