use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::StorageConfig;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageLayout {
    pub data_root: PathBuf,
    pub config_root: PathBuf,
    pub cache_root: PathBuf,
    pub titles_dir: PathBuf,
    pub database_dir: PathBuf,
    pub shared_profiles_dir: PathBuf,
    pub global_cache_dir: PathBuf,
}

impl StorageLayout {
    #[must_use]
    pub fn from_config(config_root: impl Into<PathBuf>, storage: &StorageConfig) -> Self {
        let config_root = config_root.into();
        let data_root = storage.data_root.clone();
        let global_cache_dir = storage.cache_folder.clone();
        Self {
            titles_dir: data_root.join("titles"),
            database_dir: data_root.join("database"),
            shared_profiles_dir: storage.profile_folder.clone(),
            cache_root: global_cache_dir
                .parent()
                .map_or_else(|| global_cache_dir.clone(), Path::to_path_buf),
            global_cache_dir,
            data_root,
            config_root,
        }
    }

    #[must_use]
    pub fn title_dir(&self, title_id: &str) -> PathBuf {
        self.titles_dir.join(title_id)
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.database_dir.join("titles.sqlite3")
    }

    pub fn ensure_base_dirs(&self) -> Result<(), StorageError> {
        for path in [
            &self.data_root,
            &self.config_root,
            &self.cache_root,
            &self.titles_dir,
            &self.database_dir,
            &self.shared_profiles_dir,
            &self.global_cache_dir,
        ] {
            fs::create_dir_all(path).map_err(|source| StorageError::CreateDir {
                path: path.clone(),
                source,
            })?;
        }

        Ok(())
    }

    pub fn ensure_title_dirs(&self, title_id: &str) -> Result<PathBuf, StorageError> {
        let root = self.title_dir(title_id);
        for relative in REQUIRED_TITLE_DIRS {
            let path = root.join(relative);
            fs::create_dir_all(&path).map_err(|source| StorageError::CreateDir {
                path: path.clone(),
                source,
            })?;
        }
        Ok(root)
    }
}

pub const REQUIRED_TITLE_DIRS: &[&str] = &[
    "",
    "content",
    "versions",
    "updates",
    "dlc",
    "cache",
    "cache/cpu",
    "cache/jit-promoted",
    "cache/shaders",
    "cache/pipelines",
    "cache/rollback",
    "profiles",
    "reports",
    "logs",
    "crash",
    "inspector",
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to create storage directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::config::StorageConfig;

    use super::StorageLayout;

    #[test]
    fn storage_layout_uses_deterministic_title_folder() {
        let root = tempdir().expect("temp dir should be created");
        let config_root = root.path().join("config");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(config_root, &storage);

        assert_eq!(
            layout.title_dir("0100ABCD12345678"),
            root.path()
                .join("data")
                .join("titles")
                .join("0100ABCD12345678")
        );
    }

    #[test]
    fn ensure_title_dirs_creates_required_layout() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);

        let title_root = layout
            .ensure_title_dirs("0100ABCD12345678")
            .expect("title dirs should be created");

        assert!(title_root.join("cache/cpu").is_dir());
        assert!(title_root.join("profiles").is_dir());
        assert!(title_root.join("inspector").is_dir());
    }
}
