use anyhow::{Context, Result};

const SERVICE: &str = "vvault";

fn entry(id: &str) -> Result<keyring::Entry> {
    uuid::Uuid::parse_str(id).context("invalid account id")?;
    keyring::Entry::new(SERVICE, id).context("the OS keychain is not available")
}

/// Stores an account API key in the OS keychain.
pub fn set(id: &str, secret: &str) -> Result<()> {
    let secret = secret.trim();
    if secret.is_empty() || secret.chars().any(char::is_control) {
        anyhow::bail!("the API key is empty or contains control characters");
    }
    entry(id)?
        .set_password(secret)
        .context("could not save the key in the OS keychain")
}

/// Reads an account API key from the OS keychain, if one was saved.
pub fn get(id: &str) -> Result<Option<String>> {
    match entry(id)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("could not read the key from the OS keychain"),
    }
}

/// Removes an account API key from the OS keychain; absent keys are fine.
pub fn delete(id: &str) -> Result<()> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error).context("could not remove the key from the OS keychain"),
    }
}
