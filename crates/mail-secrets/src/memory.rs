//! In-memory secret store for tests (`fake` feature). Never used by the app.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use crate::{Secret, SecretStore, SecretsError};

#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<BTreeMap<String, Secret>>,
}

impl MemoryStore {
    /// Every key currently stored, for assertions.
    pub fn keys(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Secret>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SecretStore for MemoryStore {
    fn set(&self, key: &str, value: &Secret) -> Result<(), SecretsError> {
        self.lock().insert(key.to_string(), value.clone());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Secret>, SecretsError> {
        Ok(self.lock().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<(), SecretsError> {
        self.lock().remove(key);
        Ok(())
    }
}
