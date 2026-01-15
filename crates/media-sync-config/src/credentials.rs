use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use toml;

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsData {
    #[serde(flatten)]
    data: HashMap<String, String>,
}

pub struct CredentialStore {
    path: PathBuf,
    credentials: HashMap<String, String>,
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            credentials: HashMap::new(),
        }
    }

    pub fn load(&mut self) -> Result<()> {
        if self.path.exists() {
            let content = std::fs::read_to_string(&self.path)?;
            let creds_data: CredentialsData = toml::from_str(&content)?;
            self.credentials = creds_data.data;
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let creds_data = CredentialsData {
            data: self.credentials.clone(),
        };
        let content = toml::to_string_pretty(&creds_data)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.credentials.get(key)
    }

    pub fn set(&mut self, key: String, value: String) {
        self.credentials.insert(key, value);
    }

    pub fn remove(&mut self, key: &str) {
        self.credentials.remove(key);
    }

    // Convenience methods for specific credentials
    pub fn get_imdb_password(&self) -> Option<&String> {
        self.get("imdb_password")
    }

    pub fn set_imdb_password(&mut self, password: String) {
        self.set("imdb_password".to_string(), password);
    }

    pub fn get_trakt_access_token(&self) -> Option<&String> {
        self.get("trakt_access_token")
    }

    pub fn set_trakt_access_token(&mut self, token: String) {
        self.set("trakt_access_token".to_string(), token);
    }

    pub fn get_trakt_refresh_token(&self) -> Option<&String> {
        self.get("trakt_refresh_token")
    }

    pub fn set_trakt_refresh_token(&mut self, token: String) {
        self.set("trakt_refresh_token".to_string(), token);
    }

    pub fn get_trakt_token_expires(&self) -> Option<DateTime<Utc>> {
        self.get("trakt_token_expires")
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    pub fn set_trakt_token_expires(&mut self, expires: DateTime<Utc>) {
        self.set("trakt_token_expires".to_string(), expires.to_rfc3339());
    }

    pub fn get_imdb_reviews_last_submitted(&self) -> Option<DateTime<Utc>> {
        self.get("imdb_reviews_last_submitted_date")
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    pub fn set_imdb_reviews_last_submitted(&mut self, date: DateTime<Utc>) {
        self.set(
            "imdb_reviews_last_submitted_date".to_string(),
            date.to_rfc3339(),
        );
    }

    // Simkl credential methods
    pub fn get_simkl_access_token(&self) -> Option<&String> {
        self.get("simkl_access_token")
    }

    pub fn set_simkl_access_token(&mut self, token: String) {
        self.set("simkl_access_token".to_string(), token);
    }

    pub fn get_simkl_refresh_token(&self) -> Option<&String> {
        self.get("simkl_refresh_token")
    }

    pub fn set_simkl_refresh_token(&mut self, token: String) {
        self.set("simkl_refresh_token".to_string(), token);
    }

    pub fn get_simkl_token_expires(&self) -> Option<DateTime<Utc>> {
        self.get("simkl_token_expires")
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    pub fn set_simkl_token_expires(&mut self, expires: DateTime<Utc>) {
        self.set("simkl_token_expires".to_string(), expires.to_rfc3339());
    }

    // Plex credential methods
    pub fn get_plex_token(&self) -> Option<&String> {
        self.get("plex_token")
    }

    pub fn set_plex_token(&mut self, token: String) {
        self.set("plex_token".to_string(), token);
    }

    // Generic timestamp storage methods
    pub fn get_last_sync_timestamp(&self, source: &str, data_type: &str) -> Option<DateTime<Utc>> {
        let key = format!("{}_last_sync_{}", source, data_type);
        self.get(&key)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    pub fn set_last_sync_timestamp(&mut self, source: &str, data_type: &str, timestamp: DateTime<Utc>) {
        let key = format!("{}_last_sync_{}", source, data_type);
        self.set(key, timestamp.to_rfc3339());
    }

    // Simkl-specific: Store full activities JSON for comparison
    pub fn get_simkl_last_activities(&self) -> Option<String> {
        self.get("simkl_last_activities").cloned()
    }

    pub fn set_simkl_last_activities(&mut self, activities_json: String) {
        self.set("simkl_last_activities".to_string(), activities_json);
    }

    // Helper method to get all keys (for clearing timestamps)
    pub fn get_all_keys(&self) -> Vec<String> {
        self.credentials.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_credential_store_load_and_save() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        let mut store = CredentialStore::new(path.clone());
        store.set_imdb_password("test_password".to_string());
        store.set_trakt_access_token("test_token".to_string());
        store.save().unwrap();

        let mut loaded_store = CredentialStore::new(path);
        loaded_store.load().unwrap();
        assert_eq!(loaded_store.get_imdb_password(), Some(&"test_password".to_string()));
        assert_eq!(loaded_store.get_trakt_access_token(), Some(&"test_token".to_string()));
    }

    #[test]
    fn test_credential_store_trakt_token_expires() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        let mut store = CredentialStore::new(path.clone());
        let expires = Utc::now() + chrono::Duration::hours(1);
        store.set_trakt_token_expires(expires);
        store.save().unwrap();

        let mut loaded_store = CredentialStore::new(path);
        loaded_store.load().unwrap();
        let loaded_expires = loaded_store.get_trakt_token_expires().unwrap();
        // Allow 1 second difference for serialization
        assert!((loaded_expires - expires).num_seconds().abs() < 2);
    }

    #[test]
    fn test_credential_store_remove() {
        let mut store = CredentialStore::new(PathBuf::from("/tmp/test"));
        store.set("key1".to_string(), "value1".to_string());
        store.set("key2".to_string(), "value2".to_string());
        
        assert_eq!(store.get("key1"), Some(&"value1".to_string()));
        store.remove("key1");
        assert_eq!(store.get("key1"), None);
        assert_eq!(store.get("key2"), Some(&"value2".to_string()));
    }
}

