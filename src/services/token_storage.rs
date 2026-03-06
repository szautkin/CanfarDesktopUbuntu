use keyring::Entry;

const SERVICE_NAME: &str = "canfar-verbinal";
const TOKEN_KEY: &str = "auth-token";
const USERNAME_KEY: &str = "username";

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

    pub fn clear() {
        if let Ok(entry) = Entry::new(SERVICE_NAME, TOKEN_KEY) {
            let _ = entry.delete_credential();
        }
        if let Ok(entry) = Entry::new(SERVICE_NAME, USERNAME_KEY) {
            let _ = entry.delete_credential();
        }
    }
}
