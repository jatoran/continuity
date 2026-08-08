//! SQLite operations for the machine-local known-vault registry.

use std::path::Path;

use rusqlite::params;

use crate::{Error, KnownVault, Store};

impl Store {
    /// Insert or refresh a known vault, preserving its existing pin state.
    pub fn upsert_known_vault(&self, vault: &KnownVault) -> Result<(), Error> {
        self.conn().execute(
            "INSERT INTO known_vaults(root_path, display_name, pinned, last_opened_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(root_path) DO UPDATE SET
               display_name = excluded.display_name,
               last_opened_ms = excluded.last_opened_ms",
            params![
                vault.root_path.to_string_lossy(),
                vault.display_name,
                i64::from(vault.pinned),
                vault.last_opened_ms,
            ],
        )?;
        Ok(())
    }

    /// List known vaults with pinned entries first and recency descending.
    pub fn list_known_vaults(&self) -> Result<Vec<KnownVault>, Error> {
        let mut statement = self.conn().prepare(
            "SELECT root_path, display_name, pinned, last_opened_ms
             FROM known_vaults
             ORDER BY pinned DESC, last_opened_ms DESC, display_name COLLATE NOCASE ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(KnownVault {
                root_path: row.get::<_, String>(0)?.into(),
                display_name: row.get(1)?,
                pinned: row.get::<_, i64>(2)? != 0,
                last_opened_ms: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
    }

    /// Change the pin state for one row.
    pub fn set_known_vault_pinned(&self, root_path: &Path, pinned: bool) -> Result<bool, Error> {
        let changed = self.conn().execute(
            "UPDATE known_vaults SET pinned = ?2 WHERE root_path = ?1",
            params![root_path.to_string_lossy(), i64::from(pinned)],
        )?;
        Ok(changed > 0)
    }

    /// Remove one launcher-history row.
    pub fn remove_known_vault(&self, root_path: &Path) -> Result<bool, Error> {
        let changed = self.conn().execute(
            "DELETE FROM known_vaults WHERE root_path = ?1",
            params![root_path.to_string_lossy()],
        )?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(name: &str, pinned: bool, last_opened_ms: i64) -> KnownVault {
        KnownVault {
            root_path: format!(r"C:\notes\{name}").into(),
            display_name: name.into(),
            pinned,
            last_opened_ms,
        }
    }

    #[test]
    fn known_vaults_are_pinned_then_recent() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_known_vault(&vault("older", false, 10))
            .unwrap();
        store
            .upsert_known_vault(&vault("newer", false, 20))
            .unwrap();
        store.upsert_known_vault(&vault("pinned", true, 1)).unwrap();
        let rows = store.list_known_vaults().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.display_name.as_str())
                .collect::<Vec<_>>(),
            vec!["pinned", "newer", "older"]
        );
    }

    #[test]
    fn refresh_preserves_pin_state() {
        let store = Store::open_in_memory().unwrap();
        let mut row = vault("work", false, 1);
        store.upsert_known_vault(&row).unwrap();
        store.set_known_vault_pinned(&row.root_path, true).unwrap();
        row.last_opened_ms = 2;
        store.upsert_known_vault(&row).unwrap();
        assert!(store.list_known_vaults().unwrap()[0].pinned);
    }
}
