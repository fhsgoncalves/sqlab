use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use keyring_core::{Entry, Error, set_default_store};

const SERVICE: &str = "zql";
const PASSWORDS_ACCOUNT: &str = "data-source-passwords";

static STORE_INIT: OnceLock<Result<(), String>> = OnceLock::new();
static PASSWORD_CACHE: OnceLock<Mutex<Option<HashMap<String, String>>>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential store unavailable: {0}")]
    StoreUnavailable(String),
    #[error("credential data is invalid: {0}")]
    InvalidData(#[from] serde_json::Error),
    #[error(transparent)]
    Keyring(#[from] Error),
}

pub fn recovery_error_message(error: &CredentialError) -> String {
    format!(
        "Password recovery failed because zql could not access the system keychain.\n\n{}",
        error
    )
}

pub fn saving_error_message(error: &CredentialError) -> String {
    format!(
        "Password saving failed because zql could not access the system keychain.\n\n{}",
        error
    )
}

pub fn load_password(account: &str) -> Result<Option<String>, CredentialError> {
    if account.is_empty() {
        return Ok(None);
    }

    Ok(load_passwords()?.get(account).cloned())
}

pub fn load_passwords() -> Result<HashMap<String, String>, CredentialError> {
    if let Some(passwords) = password_cache()
        .lock()
        .expect("password cache poisoned")
        .clone()
    {
        return Ok(passwords);
    }

    let passwords = match entry(PASSWORDS_ACCOUNT)?.get_password() {
        Ok(content) => serde_json::from_str(&content)?,
        Err(Error::NoEntry) => HashMap::new(),
        Err(error) => return Err(error.into()),
    };

    *password_cache().lock().expect("password cache poisoned") = Some(passwords.clone());
    Ok(passwords)
}

pub fn save_passwords_from_configs<'a>(
    configs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), CredentialError> {
    let mut passwords = load_passwords()?;

    for (account, password) in configs {
        if !account.is_empty() && !password.is_empty() {
            passwords.insert(account.to_string(), password.to_string());
        }
    }

    save_password_map(passwords)
}

pub fn delete_password(account: &str) -> Result<(), CredentialError> {
    if account.is_empty() {
        return Ok(());
    }

    let mut passwords = load_passwords()?;
    passwords.remove(account);
    save_password_map(passwords)
}

fn save_password_map(passwords: HashMap<String, String>) -> Result<(), CredentialError> {
    if passwords.is_empty() {
        match entry(PASSWORDS_ACCOUNT)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => {}
            Err(error) => return Err(error.into()),
        }
    } else {
        let content = serde_json::to_string(&passwords)?;
        entry(PASSWORDS_ACCOUNT)?.set_password(&content)?;
    }

    *password_cache().lock().expect("password cache poisoned") = Some(passwords);
    Ok(())
}

fn entry(account: &str) -> Result<Entry, CredentialError> {
    ensure_store()?;
    Ok(Entry::new(SERVICE, account)?)
}

fn password_cache() -> &'static Mutex<Option<HashMap<String, String>>> {
    PASSWORD_CACHE.get_or_init(|| Mutex::new(None))
}

fn ensure_store() -> Result<(), CredentialError> {
    match STORE_INIT.get_or_init(|| configure_native_store().map_err(|error| error.to_string())) {
        Ok(()) => Ok(()),
        Err(error) => Err(CredentialError::StoreUnavailable(error.clone())),
    }
}

fn configure_native_store() -> keyring_core::Result<()> {
    let config = HashMap::new();

    #[cfg(target_os = "macos")]
    {
        use apple_native_keyring_store::keychain::Store;

        set_default_store(Store::new_with_configuration(&config)?);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use windows_native_keyring_store::Store;

        set_default_store(Store::new_with_configuration(&config)?);
        return Ok(());
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use zbus_secret_service_keyring_store::Store;

        set_default_store(Store::new_with_configuration(&config)?);
        return Ok(());
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )))]
    {
        Err(Error::NotSupportedByStore(
            "zql does not have a native credential store for this platform".to_string(),
        ))
    }
}
