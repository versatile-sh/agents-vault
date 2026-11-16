//! Account ledger core: one isolated home per account, secrets in the OS keychain,
//! and the harness CLI as the only owner of its own OAuth tokens.

pub mod claude;
pub mod codex;
pub mod kit;
pub mod launch;
pub mod ledger;
pub mod login;
pub mod limits;
pub mod model;
pub mod proc;
pub mod secrets;
pub mod status;
pub mod store;
pub mod terminal;

pub use anyhow::Result;
pub use model::{Account, Auth, Config, Harness, Kit, Provider, Tool};
pub use store::Store;
