use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use nx86_core::storage::{StorageError, StorageLayout};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct TitleDatabase {
    layout: StorageLayout,
    connection: Connection,
}

impl TitleDatabase {
    pub fn open(layout: StorageLayout) -> Result<Self, TitleDbError> {
        layout.ensure_base_dirs()?;
        let connection = Connection::open(layout.database_path()).map_err(TitleDbError::Sqlite)?;
        let database = Self { layout, connection };
        database.migrate()?;
        Ok(database)
    }

    #[must_use]
    pub const fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    pub fn create_placeholder(
        &self,
        title_id: TitleId,
        display_name: impl Into<String>,
    ) -> Result<TitleEntry, TitleDbError> {
        let now = unix_now();
        let title_root = self.layout.ensure_title_dirs(title_id.as_str())?;
        let entry = TitleEntry {
            title_id,
            display_name: display_name.into(),
            source_kind: TitleSourceKind::Placeholder,
            content_path: None,
            folder_path: title_root,
            created_at_unix_secs: now,
            updated_at_unix_secs: now,
        };

        self.insert_title(&entry)?;
        write_sidecars(&entry)?;
        Ok(entry)
    }

    /// Create a title backed by a synthetic test program.
    ///
    /// The caller-supplied `program_toml` is persisted verbatim under the
    /// title's `content/` directory; parsing of the synthetic format stays in
    /// the higher layers. Only the project's own synthetic format flows through
    /// here — never copyrighted game dumps or firmware.
    pub fn create_synthetic_title(
        &self,
        title_id: TitleId,
        display_name: impl Into<String>,
        program_toml: &str,
    ) -> Result<TitleEntry, TitleDbError> {
        let now = unix_now();
        let title_root = self.layout.ensure_title_dirs(title_id.as_str())?;
        let content_path = title_root.join(SYNTHETIC_CONTENT_FILE);
        let entry = TitleEntry {
            title_id,
            display_name: display_name.into(),
            source_kind: TitleSourceKind::Synthetic,
            content_path: Some(content_path.clone()),
            folder_path: title_root,
            created_at_unix_secs: now,
            updated_at_unix_secs: now,
        };

        // Insert first, so a duplicate title id fails before any file is written
        // and never clobbers an existing title's content.
        self.insert_title(&entry)?;
        fs::write(&content_path, program_toml).map_err(|source| TitleDbError::WriteContent {
            path: content_path,
            source,
        })?;
        write_sidecars(&entry)?;
        Ok(entry)
    }

    /// Create a title backed by a simple local homebrew module descriptor.
    ///
    /// The caller-supplied TOML is persisted verbatim under the title's
    /// `content/` directory. Validation and loading live in `nx86-import`; the
    /// title database only records this as user-provided homebrew content.
    pub fn create_homebrew_title(
        &self,
        title_id: TitleId,
        display_name: impl Into<String>,
        module_toml: &str,
    ) -> Result<TitleEntry, TitleDbError> {
        let now = unix_now();
        let title_root = self.layout.ensure_title_dirs(title_id.as_str())?;
        let content_path = title_root.join(HOMEBREW_CONTENT_FILE);
        let entry = TitleEntry {
            title_id,
            display_name: display_name.into(),
            source_kind: TitleSourceKind::Homebrew,
            content_path: Some(content_path.clone()),
            folder_path: title_root,
            created_at_unix_secs: now,
            updated_at_unix_secs: now,
        };

        self.insert_title(&entry)?;
        fs::write(&content_path, module_toml).map_err(|source| TitleDbError::WriteContent {
            path: content_path,
            source,
        })?;
        write_sidecars(&entry)?;
        Ok(entry)
    }

    /// Read a title's stored content (e.g. the synthetic program TOML), if any.
    pub fn read_content(&self, entry: &TitleEntry) -> Result<Option<String>, TitleDbError> {
        let Some(content_path) = entry.content_path.as_ref() else {
            return Ok(None);
        };
        let contents =
            fs::read_to_string(content_path).map_err(|source| TitleDbError::ReadContent {
                path: content_path.clone(),
                source,
            })?;
        Ok(Some(contents))
    }

    fn insert_title(&self, entry: &TitleEntry) -> Result<(), TitleDbError> {
        self.connection.execute(
            "INSERT INTO titles (
                title_id,
                display_name,
                source_kind,
                content_path,
                folder_path,
                created_at_unix_secs,
                updated_at_unix_secs
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.title_id.as_str(),
                entry.display_name,
                entry.source_kind.as_str(),
                entry
                    .content_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                entry.folder_path.display().to_string(),
                entry.created_at_unix_secs,
                entry.updated_at_unix_secs,
            ],
        )?;
        Ok(())
    }

    pub fn list_titles(&self) -> Result<Vec<TitleEntry>, TitleDbError> {
        let mut statement = self.connection.prepare(
            "SELECT title_id,
                    display_name,
                    source_kind,
                    content_path,
                    folder_path,
                    created_at_unix_secs,
                    updated_at_unix_secs
             FROM titles
             ORDER BY display_name COLLATE NOCASE, title_id",
        )?;

        let rows = statement.query_map([], row_to_title_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn get_title(&self, title_id: &TitleId) -> Result<Option<TitleEntry>, TitleDbError> {
        self.connection
            .query_row(
                "SELECT title_id,
                        display_name,
                        source_kind,
                        content_path,
                        folder_path,
                        created_at_unix_secs,
                        updated_at_unix_secs
                 FROM titles
                 WHERE title_id = ?1",
                params![title_id.as_str()],
                row_to_title_entry,
            )
            .optional()
            .map_err(TitleDbError::Sqlite)
    }

    pub fn read_sidecars(&self, title_id: &TitleId) -> Result<TitleSidecars, TitleDbError> {
        let entry = self
            .get_title(title_id)?
            .ok_or_else(|| TitleDbError::MissingTitle {
                title_id: title_id.to_string(),
            })?;
        read_sidecars(&entry)
    }

    pub fn rewrite_sidecars(&self, title_id: &TitleId) -> Result<TitleSidecars, TitleDbError> {
        let entry = self
            .get_title(title_id)?
            .ok_or_else(|| TitleDbError::MissingTitle {
                title_id: title_id.to_string(),
            })?;
        write_sidecars(&entry)?;
        read_sidecars(&entry)
    }

    fn migrate(&self) -> Result<(), TitleDbError> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS migrations (
                version INTEGER PRIMARY KEY,
                applied_at_unix_secs INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS titles (
                title_id TEXT PRIMARY KEY NOT NULL,
                display_name TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                content_path TEXT,
                folder_path TEXT NOT NULL,
                created_at_unix_secs INTEGER NOT NULL,
                updated_at_unix_secs INTEGER NOT NULL
            );
            ",
        )?;

        self.connection.execute(
            "INSERT OR IGNORE INTO migrations (version, applied_at_unix_secs) VALUES (?1, ?2)",
            params![1_i64, unix_now()],
        )?;

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TitleEntry {
    pub title_id: TitleId,
    pub display_name: String,
    pub source_kind: TitleSourceKind,
    pub content_path: Option<PathBuf>,
    pub folder_path: PathBuf,
    pub created_at_unix_secs: i64,
    pub updated_at_unix_secs: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(try_from = "String", into = "String")]
pub struct TitleId(String);

impl TitleId {
    pub fn parse(source: &str) -> Result<Self, TitleDbError> {
        let normalized = source.trim().to_ascii_uppercase();
        if normalized.len() != 16 || !normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(TitleDbError::InvalidTitleId {
                value: source.to_owned(),
            });
        }
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TitleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for TitleId {
    type Error = TitleDbError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TitleId> for String {
    fn from(value: TitleId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TitleSourceKind {
    Placeholder,
    Synthetic,
    Homebrew,
}

impl TitleSourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Placeholder => "placeholder",
            Self::Synthetic => "synthetic",
            Self::Homebrew => "homebrew",
        }
    }
}

impl TryFrom<&str> for TitleSourceKind {
    type Error = TitleDbError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "placeholder" => Ok(Self::Placeholder),
            "synthetic" => Ok(Self::Synthetic),
            "homebrew" => Ok(Self::Homebrew),
            _ => Err(TitleDbError::InvalidSourceKind {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TitleSidecars {
    pub metadata: TitleMetaSidecar,
    pub settings: TitleSettingsSidecar,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TitleMetaSidecar {
    pub title_id: TitleId,
    pub display_name: String,
    pub source_kind: TitleSourceKind,
    pub created_at_unix_secs: i64,
    pub updated_at_unix_secs: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TitleSettingsSidecar {
    pub import_enabled: bool,
    pub storage_mode: String,
}

fn row_to_title_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<TitleEntry> {
    let title_id: String = row.get(0)?;
    let source_kind: String = row.get(2)?;
    let content_path: Option<String> = row.get(3)?;
    let created_at_unix_secs: i64 = row.get(5)?;
    let updated_at_unix_secs: i64 = row.get(6)?;

    let title_id = TitleId::parse(&title_id).map_err(to_sql_error)?;
    let source_kind = TitleSourceKind::try_from(source_kind.as_str()).map_err(to_sql_error)?;

    Ok(TitleEntry {
        title_id,
        display_name: row.get(1)?,
        source_kind,
        content_path: content_path.map(PathBuf::from),
        folder_path: PathBuf::from(row.get::<_, String>(4)?),
        created_at_unix_secs,
        updated_at_unix_secs,
    })
}

fn write_sidecars(entry: &TitleEntry) -> Result<(), TitleDbError> {
    let meta = TitleMetaSidecar {
        title_id: entry.title_id.clone(),
        display_name: entry.display_name.clone(),
        source_kind: entry.source_kind,
        created_at_unix_secs: entry.created_at_unix_secs,
        updated_at_unix_secs: entry.updated_at_unix_secs,
    };
    let settings = TitleSettingsSidecar {
        import_enabled: false,
        storage_mode: entry.source_kind.as_str().to_owned(),
    };

    write_toml(entry.folder_path.join("title.nxmeta"), &meta)?;
    write_toml(entry.folder_path.join("settings.toml"), &settings)?;
    Ok(())
}

fn read_sidecars(entry: &TitleEntry) -> Result<TitleSidecars, TitleDbError> {
    Ok(TitleSidecars {
        metadata: read_toml(entry.folder_path.join("title.nxmeta"))?,
        settings: read_toml(entry.folder_path.join("settings.toml"))?,
    })
}

fn write_toml(path: impl AsRef<Path>, value: &impl Serialize) -> Result<(), TitleDbError> {
    let path = path.as_ref();
    let serialized = toml::to_string_pretty(value).map_err(TitleDbError::TomlSerialize)?;
    fs::write(path, serialized).map_err(|source| TitleDbError::WriteSidecar {
        path: path.to_path_buf(),
        source,
    })
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: impl AsRef<Path>) -> Result<T, TitleDbError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| TitleDbError::ReadSidecar {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&source).map_err(|source| TitleDbError::TomlDeserialize {
        path: path.to_path_buf(),
        source,
    })
}

/// Title-relative path where synthetic program content is persisted.
const SYNTHETIC_CONTENT_FILE: &str = "content/program.nxsynth.toml";
/// Title-relative path where simple homebrew module descriptors are persisted.
const HOMEBREW_CONTENT_FILE: &str = "content/homebrew.nxhb.toml";

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn to_sql_error(error: TitleDbError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[derive(Debug, Error)]
pub enum TitleDbError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid title id `{value}`; expected 16 hexadecimal characters")]
    InvalidTitleId { value: String },
    #[error("invalid title source kind `{value}`")]
    InvalidSourceKind { value: String },
    #[error("title `{title_id}` does not exist")]
    MissingTitle { title_id: String },
    #[error("failed to read title sidecar {path}: {source}")]
    ReadSidecar {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to deserialize title sidecar {path}: {source}")]
    TomlDeserialize {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to serialize title sidecar: {0}")]
    TomlSerialize(toml::ser::Error),
    #[error("failed to write title sidecar {path}: {source}")]
    WriteSidecar {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write title content {path}: {source}")]
    WriteContent {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read title content {path}: {source}")]
    ReadContent {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use nx86_core::{config::StorageConfig, storage::StorageLayout};
    use tempfile::tempdir;

    use super::{TitleDatabase, TitleDbError, TitleId, TitleSourceKind};

    #[test]
    fn title_id_normalizes_uppercase() {
        let title_id = TitleId::parse("0100abcd12345678").expect("title id should parse");

        assert_eq!(title_id.as_str(), "0100ABCD12345678");
    }

    #[test]
    fn invalid_title_id_is_rejected() {
        assert!(TitleId::parse("not-a-title").is_err());
    }

    #[test]
    fn database_creates_lists_and_survives_reopen() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");

        {
            let database = TitleDatabase::open(layout.clone()).expect("database should open");
            let entry = database
                .create_placeholder(title_id.clone(), "Placeholder Title")
                .expect("placeholder should be created");

            assert_eq!(entry.folder_path, layout.title_dir(title_id.as_str()));
            assert!(entry.folder_path.join("title.nxmeta").is_file());
            assert!(entry.folder_path.join("settings.toml").is_file());
            assert!(entry.folder_path.join("cache/shaders").is_dir());
        }

        let database = TitleDatabase::open(layout).expect("database should reopen");
        let titles = database.list_titles().expect("titles should list");

        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].title_id, title_id);
        assert_eq!(titles[0].display_name, "Placeholder Title");
    }

    #[test]
    fn sidecars_read_and_rewrite_from_database_entry() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");
        let database = TitleDatabase::open(layout).expect("database should open");
        let entry = database
            .create_placeholder(title_id.clone(), "Placeholder Title")
            .expect("placeholder should be created");

        let sidecars = database
            .read_sidecars(&title_id)
            .expect("sidecars should read");
        assert_eq!(sidecars.metadata.title_id, title_id);
        assert_eq!(sidecars.metadata.display_name, "Placeholder Title");
        assert!(!sidecars.settings.import_enabled);
        assert_eq!(sidecars.settings.storage_mode, "placeholder");

        std::fs::write(entry.folder_path.join("settings.toml"), "not = [valid")
            .expect("sidecar should be writable");
        let error = database
            .read_sidecars(&title_id)
            .expect_err("corrupt sidecar should fail");
        assert!(matches!(error, TitleDbError::TomlDeserialize { .. }));

        let rewritten = database
            .rewrite_sidecars(&title_id)
            .expect("sidecars should rewrite");
        assert_eq!(rewritten, sidecars);
    }

    #[test]
    fn synthetic_title_persists_and_reads_back_content() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");
        let program = "[program]\narm64-hex = \"20 00 80 D2\"\n";

        let database = TitleDatabase::open(layout.clone()).expect("database should open");
        let entry = database
            .create_synthetic_title(title_id.clone(), "Synthetic Title", program)
            .expect("synthetic title should be created");

        assert_eq!(entry.source_kind, TitleSourceKind::Synthetic);
        let content_path = entry
            .content_path
            .as_ref()
            .expect("synthetic title carries content");
        assert!(content_path.is_file());

        let contents = database
            .read_content(&entry)
            .expect("content should read")
            .expect("synthetic title has content");
        assert_eq!(contents, program);

        // The source kind survives a round-trip through the database and sidecar.
        let reopened = TitleDatabase::open(layout).expect("database should reopen");
        let listed = reopened.list_titles().expect("titles should list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].source_kind, TitleSourceKind::Synthetic);
        let sidecars = reopened
            .read_sidecars(&title_id)
            .expect("sidecars should read");
        assert_eq!(sidecars.settings.storage_mode, "synthetic");
    }

    #[test]
    fn homebrew_title_persists_and_reads_back_content() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");
        let module = "[metadata]\nname = \"Exit\"\n[program]\narm64-hex = \"01 00 00 D4\"\n";

        let database = TitleDatabase::open(layout.clone()).expect("database should open");
        let entry = database
            .create_homebrew_title(title_id.clone(), "Homebrew Title", module)
            .expect("homebrew title should be created");

        assert_eq!(entry.source_kind, TitleSourceKind::Homebrew);
        let content_path = entry
            .content_path
            .as_ref()
            .expect("homebrew title carries content");
        assert!(content_path.ends_with("content/homebrew.nxhb.toml"));
        assert!(content_path.is_file());
        assert_eq!(
            database
                .read_content(&entry)
                .expect("content should read")
                .expect("homebrew title has content"),
            module
        );

        let reopened = TitleDatabase::open(layout).expect("database should reopen");
        let listed = reopened.list_titles().expect("titles should list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].source_kind, TitleSourceKind::Homebrew);
        let sidecars = reopened
            .read_sidecars(&title_id)
            .expect("sidecars should read");
        assert_eq!(sidecars.settings.storage_mode, "homebrew");
    }

    #[test]
    fn duplicate_title_id_is_rejected_without_clobbering_content() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");
        let original = "[program]\narm64-hex = \"20 00 80 D2\"\n";

        let database = TitleDatabase::open(layout).expect("database should open");
        let entry = database
            .create_synthetic_title(title_id.clone(), "Original", original)
            .expect("first title should be created");

        // A second create with the same id must fail at the database insert and
        // leave the already-stored content untouched (insert precedes the write).
        let error = database
            .create_synthetic_title(
                title_id,
                "Replacement",
                "[program]\narm64-hex = \"FF FF\"\n",
            )
            .expect_err("duplicate title id should be rejected");
        assert!(matches!(error, TitleDbError::Sqlite(_)));

        let contents = database
            .read_content(&entry)
            .expect("content should read")
            .expect("title still has content");
        assert_eq!(contents, original);
    }

    #[test]
    fn placeholder_title_has_no_content() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");
        let database = TitleDatabase::open(layout).expect("database should open");
        let entry = database
            .create_placeholder(title_id, "Placeholder Title")
            .expect("placeholder should be created");

        assert!(entry.content_path.is_none());
        assert_eq!(
            database.read_content(&entry).expect("read should succeed"),
            None
        );
    }

    #[test]
    fn missing_title_sidecars_are_reported() {
        let root = tempdir().expect("temp dir should be created");
        let storage =
            StorageConfig::from_roots(root.path().join("data"), root.path().join("cache"));
        let layout = StorageLayout::from_config(root.path().join("config"), &storage);
        let title_id = TitleId::parse("0100ABCD12345678").expect("title id should parse");
        let database = TitleDatabase::open(layout).expect("database should open");

        let error = database
            .read_sidecars(&title_id)
            .expect_err("missing title should fail");

        assert_eq!(error.to_string(), "title `0100ABCD12345678` does not exist");
    }
}
