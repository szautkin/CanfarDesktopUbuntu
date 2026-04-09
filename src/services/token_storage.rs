use keyring::Entry;

const SERVICE_NAME: &str = "canfar-verbinal";
const TOKEN_KEY: &str = "auth-token";
const USERNAME_KEY: &str = "username";
const PASSWORD_KEY: &str = "password";

pub struct TokenStorage;

impl TokenStorage {
    pub fn save_token(token: &str) -> Result<(), String> {
        let entry = Entry::new(SERVICE_NAME, TOKEN_KEY).map_err(|e| e.to_string())?;
        entry.set_password(token).map_err(|e| e.to_string())
    }

    pub fn get_token() -> Option<String> {
        let entry = Entry::new(SERVICE_NAME, TOKEN_KEY).ok()?;
        entry.get_password().ok()
    }

    pub fn save_username(username: &str) -> Result<(), String> {
        let entry = Entry::new(SERVICE_NAME, USERNAME_KEY).map_err(|e| e.to_string())?;
        entry.set_password(username).map_err(|e| e.to_string())
    }

    pub fn get_username() -> Option<String> {
        let entry = Entry::new(SERVICE_NAME, USERNAME_KEY).ok()?;
        entry.get_password().ok()
    }

    /// Store the user's password for silent re-authentication.
    /// This is best-effort; failures are silently ignored by the caller.
    pub fn save_password(password: &str) -> Result<(), String> {
        let entry = Entry::new(SERVICE_NAME, PASSWORD_KEY).map_err(|e| e.to_string())?;
        entry.set_password(password).map_err(|e| e.to_string())
    }

    pub fn get_password() -> Option<String> {
        let entry = Entry::new(SERVICE_NAME, PASSWORD_KEY).ok()?;
        entry.get_password().ok()
    }

    /// Return `(username, password)` if both are stored in the keyring.
    pub fn get_credentials() -> Option<(String, String)> {
        let username = Self::get_username()?;
        let password = Self::get_password()?;
        Some((username, password))
    }

    pub fn clear() {
        if let Ok(entry) = Entry::new(SERVICE_NAME, TOKEN_KEY) {
            let _ = entry.delete_credential();
        }
        if let Ok(entry) = Entry::new(SERVICE_NAME, USERNAME_KEY) {
            let _ = entry.delete_credential();
        }
        if let Ok(entry) = Entry::new(SERVICE_NAME, PASSWORD_KEY) {
            let _ = entry.delete_credential();
        }
    }
}
