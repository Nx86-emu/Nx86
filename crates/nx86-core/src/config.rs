use std::{
    fmt, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct AppConfig {
    pub ui: UiConfig,
    pub prototype: PrototypeConfig,
    pub first_run: FirstRunConfig,
    pub storage: StorageConfig,
    pub compiler: CompilerConfig,
    pub graphics: GraphicsConfig,
    pub input: InputConfig,
    pub profile_sharing: ProfileSharingConfig,
}

impl AppConfig {
    #[must_use]
    pub fn wizard_is_pending(&self) -> bool {
        self.first_run.phase3_wizard_pending
    }

    pub fn validate_wizard(&self) -> Result<(), WizardValidation> {
        let mut errors = Vec::new();

        if self.storage.library_folders.is_empty()
            || self
                .storage
                .library_folders
                .iter()
                .any(|path| path.as_os_str().is_empty())
        {
            errors.push(WizardValidationError::MissingLibraryFolder);
        }

        if self.storage.cache_folder.as_os_str().is_empty() {
            errors.push(WizardValidationError::MissingCacheFolder);
        }

        if self.storage.profile_folder.as_os_str().is_empty() {
            errors.push(WizardValidationError::MissingProfileFolder);
        }

        let available = available_parallelism();
        if self.compiler.compile_thread_cap == 0
            || self.compiler.compile_thread_cap > available.saturating_mul(2)
        {
            errors.push(WizardValidationError::InvalidCompileThreadCap {
                cap: self.compiler.compile_thread_cap,
                max: available.saturating_mul(2),
            });
        }

        if self.compiler.compile_thread_cap >= available
            && !self.compiler.all_core_warning_acknowledged
        {
            errors.push(WizardValidationError::AllCoreWarningNotAcknowledged);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(WizardValidation { errors })
        }
    }

    pub fn complete_first_launch(&mut self) -> Result<(), WizardValidation> {
        self.validate_wizard()?;
        self.first_run.phase3_wizard_pending = false;
        self.first_run.wizard_completed_at_unix_secs = Some(unix_now());
        self.ui.selected_screen = AppScreen::Library;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct UiConfig {
    pub selected_screen: AppScreen,
    pub theme_mode: ThemeMode,
    pub developer_mode_visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct PrototypeConfig {
    pub target_os: String,
    pub target_cpu: String,
    pub graphics_backend: String,
    pub gui_framework: String,
}

impl Default for PrototypeConfig {
    fn default() -> Self {
        Self {
            target_os: "linux".to_owned(),
            target_cpu: CpuTarget::X86_64V4.label().to_owned(),
            graphics_backend: GraphicsBackend::Vulkan.label().to_owned(),
            gui_framework: "egui".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct FirstRunConfig {
    pub phase3_wizard_pending: bool,
    pub wizard_completed_at_unix_secs: Option<u64>,
}

impl Default for FirstRunConfig {
    fn default() -> Self {
        Self {
            phase3_wizard_pending: true,
            wizard_completed_at_unix_secs: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct StorageConfig {
    pub data_root: PathBuf,
    pub library_folders: Vec<PathBuf>,
    pub cache_folder: PathBuf,
    pub profile_folder: PathBuf,
}

impl StorageConfig {
    #[must_use]
    pub fn from_roots(data_root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        let data_root = data_root.into();
        let cache_root = cache_root.into();
        Self {
            library_folders: vec![data_root.join("library")],
            cache_folder: cache_root.join("global-cache"),
            profile_folder: data_root.join("shared-profiles"),
            data_root,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        match ProjectDirs::from("", "", "Nx86") {
            Some(dirs) => Self::from_roots(dirs.data_dir(), dirs.cache_dir()),
            None => Self::from_roots(PathBuf::from("Nx86"), PathBuf::from("Nx86")),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct CompilerConfig {
    pub cpu_target: CpuTarget,
    pub compile_thread_cap: usize,
    pub all_core_warning_acknowledged: bool,
    /// Experimental Linux x86_64 block-exit patching. Software chaining remains
    /// available when this is disabled or unsupported.
    pub native_patch_chaining: bool,
    /// When true, memory mirrors get independent storage instead of sharing
    /// backing pages. Useful for debugging divergent writes. Release mode
    /// uses false so mirrors share physical backing.
    pub debug_memory_mirrors: bool,
}

impl Default for CompilerConfig {
    fn default() -> Self {
        Self {
            cpu_target: CpuTarget::X86_64V4,
            compile_thread_cap: available_parallelism(),
            all_core_warning_acknowledged: false,
            native_patch_chaining: false,
            debug_memory_mirrors: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct GraphicsConfig {
    pub backend: GraphicsBackend,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct ProfileSharingConfig {
    pub enabled: bool,
    pub upload_requires_approval: bool,
    pub download_requires_approval: bool,
}

impl Default for ProfileSharingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            upload_requires_approval: true,
            download_requires_approval: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct InputConfig {
    pub gamepad_enabled: bool,
    pub keyboard: KeyboardInputConfig,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            gamepad_enabled: true,
            keyboard: KeyboardInputConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct KeyboardInputConfig {
    pub dpad_up: KeyboardKey,
    pub dpad_down: KeyboardKey,
    pub dpad_left: KeyboardKey,
    pub dpad_right: KeyboardKey,
    pub a: KeyboardKey,
    pub b: KeyboardKey,
    pub x: KeyboardKey,
    pub y: KeyboardKey,
    pub l: KeyboardKey,
    pub r: KeyboardKey,
    pub zl: KeyboardKey,
    pub zr: KeyboardKey,
    pub plus: KeyboardKey,
    pub minus: KeyboardKey,
}

impl Default for KeyboardInputConfig {
    fn default() -> Self {
        Self {
            dpad_up: KeyboardKey::W,
            dpad_down: KeyboardKey::S,
            dpad_left: KeyboardKey::A,
            dpad_right: KeyboardKey::D,
            a: KeyboardKey::Space,
            b: KeyboardKey::J,
            x: KeyboardKey::K,
            y: KeyboardKey::U,
            l: KeyboardKey::Q,
            r: KeyboardKey::E,
            zl: KeyboardKey::Z,
            zr: KeyboardKey::X,
            plus: KeyboardKey::Enter,
            minus: KeyboardKey::Tab,
        }
    }
}

impl KeyboardInputConfig {
    #[must_use]
    pub const fn key_for(self, binding: InputBinding) -> KeyboardKey {
        match binding {
            InputBinding::DPadUp => self.dpad_up,
            InputBinding::DPadDown => self.dpad_down,
            InputBinding::DPadLeft => self.dpad_left,
            InputBinding::DPadRight => self.dpad_right,
            InputBinding::A => self.a,
            InputBinding::B => self.b,
            InputBinding::X => self.x,
            InputBinding::Y => self.y,
            InputBinding::L => self.l,
            InputBinding::R => self.r,
            InputBinding::ZL => self.zl,
            InputBinding::ZR => self.zr,
            InputBinding::Plus => self.plus,
            InputBinding::Minus => self.minus,
        }
    }

    pub fn set_key_for(&mut self, binding: InputBinding, key: KeyboardKey) {
        match binding {
            InputBinding::DPadUp => self.dpad_up = key,
            InputBinding::DPadDown => self.dpad_down = key,
            InputBinding::DPadLeft => self.dpad_left = key,
            InputBinding::DPadRight => self.dpad_right = key,
            InputBinding::A => self.a = key,
            InputBinding::B => self.b = key,
            InputBinding::X => self.x = key,
            InputBinding::Y => self.y = key,
            InputBinding::L => self.l = key,
            InputBinding::R => self.r = key,
            InputBinding::ZL => self.zl = key,
            InputBinding::ZR => self.zr = key,
            InputBinding::Plus => self.plus = key,
            InputBinding::Minus => self.minus = key,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InputBinding {
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    A,
    B,
    X,
    Y,
    L,
    R,
    ZL,
    ZR,
    Plus,
    Minus,
}

impl InputBinding {
    pub const ALL: [Self; 14] = [
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::L,
        Self::R,
        Self::ZL,
        Self::ZR,
        Self::Plus,
        Self::Minus,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DPadUp => "D-pad Up",
            Self::DPadDown => "D-pad Down",
            Self::DPadLeft => "D-pad Left",
            Self::DPadRight => "D-pad Right",
            Self::A => "A",
            Self::B => "B",
            Self::X => "X",
            Self::Y => "Y",
            Self::L => "L",
            Self::R => "R",
            Self::ZL => "ZL",
            Self::ZR => "ZR",
            Self::Plus => "Plus",
            Self::Minus => "Minus",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum KeyboardKey {
    #[default]
    Space,
    Enter,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
}

impl KeyboardKey {
    pub const ALL: [Self; 33] = [
        Self::Space,
        Self::Enter,
        Self::Tab,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::N,
        Self::O,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::U,
        Self::V,
        Self::W,
        Self::X,
        Self::Y,
        Self::Z,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Space => "Space",
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::ArrowUp => "Arrow Up",
            Self::ArrowDown => "Arrow Down",
            Self::ArrowLeft => "Arrow Left",
            Self::ArrowRight => "Arrow Right",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AppScreen {
    #[default]
    Library,
    Compile,
    Tests,
    Scheduler,
    Inspector,
    Settings,
}

impl AppScreen {
    pub const ALL: [Self; 6] = [
        Self::Library,
        Self::Compile,
        Self::Tests,
        Self::Scheduler,
        Self::Inspector,
        Self::Settings,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Compile => "Compile",
            Self::Tests => "Tests",
            Self::Scheduler => "Scheduler",
            Self::Inspector => "Inspector",
            Self::Settings => "Settings",
        }
    }
}

impl fmt::Display for AppScreen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub const ALL: [Self; 2] = [Self::Dark, Self::Light];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Nx86 Dark",
            Self::Light => "Nx86 Light",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CpuTarget {
    #[default]
    X86_64V4,
}

impl CpuTarget {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::X86_64V4 => "x86_64-v4",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GraphicsBackend {
    #[default]
    Vulkan,
}

impl GraphicsBackend {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WizardValidation {
    errors: Vec<WizardValidationError>,
}

impl WizardValidation {
    #[must_use]
    pub fn errors(&self) -> &[WizardValidationError] {
        &self.errors
    }
}

impl fmt::Display for WizardValidation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{error}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WizardValidationError {
    MissingLibraryFolder,
    MissingCacheFolder,
    MissingProfileFolder,
    InvalidCompileThreadCap { cap: usize, max: usize },
    AllCoreWarningNotAcknowledged,
}

impl fmt::Display for WizardValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLibraryFolder => formatter.write_str("choose at least one library folder"),
            Self::MissingCacheFolder => formatter.write_str("choose a cache folder"),
            Self::MissingProfileFolder => formatter.write_str("choose a profile folder"),
            Self::InvalidCompileThreadCap { cap, max } => {
                write!(
                    formatter,
                    "compile thread cap {cap} must be between 1 and {max}"
                )
            }
            Self::AllCoreWarningNotAcknowledged => {
                formatter.write_str("acknowledge the all-core compilation warning")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn for_linux_xdg() -> Result<Self, ConfigError> {
        let dirs = ProjectDirs::from("", "", "Nx86").ok_or(ConfigError::ProjectDirsUnavailable)?;
        Ok(Self::from_path(dirs.config_dir().join("config.toml")))
    }

    #[must_use]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn config_root(&self) -> PathBuf {
        self.path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    pub fn load(&self) -> Result<AppConfig, ConfigError> {
        let source = match fs::read_to_string(&self.path) {
            Ok(source) => source,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Err(ConfigError::NotFound {
                    path: self.path.clone(),
                });
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        toml::from_str(&source).map_err(|source| ConfigError::Parse {
            path: self.path.clone(),
            source,
        })
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let serialized = toml::to_string_pretty(config).map_err(ConfigError::Serialize)?;
        fs::write(&self.path, serialized).map_err(|source| ConfigError::Write {
            path: self.path.clone(),
            source,
        })
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not resolve the Nx86 config directory")]
    ProjectDirsUnavailable,
    #[error("config file does not exist: {path}")]
    NotFound { path: PathBuf },
    #[error("failed to read config file {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to create config directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to serialize config: {0}")]
    Serialize(toml::ser::Error),
    #[error("failed to write config file {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

#[must_use]
pub fn available_parallelism() -> usize {
    thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::{
        AppConfig, AppScreen, ConfigError, ConfigStore, GraphicsBackend, StorageConfig, ThemeMode,
        WizardValidationError,
    };

    #[test]
    fn defaults_match_phase_3_target() {
        let config = AppConfig::default();

        assert_eq!(config.ui.selected_screen, AppScreen::Library);
        assert_eq!(config.ui.theme_mode, ThemeMode::Dark);
        assert!(!config.ui.developer_mode_visible);
        assert_eq!(config.prototype.target_os, "linux");
        assert_eq!(config.prototype.target_cpu, "x86_64-v4");
        assert_eq!(config.prototype.graphics_backend, "vulkan");
        assert_eq!(config.prototype.gui_framework, "egui");
        assert_eq!(config.graphics.backend, GraphicsBackend::Vulkan);
        assert!(config.input.gamepad_enabled);
        assert_eq!(config.input.keyboard.dpad_up, super::KeyboardKey::W);
        assert_eq!(config.input.keyboard.a, super::KeyboardKey::Space);
        assert!(!config.compiler.native_patch_chaining);
        assert!(!config.compiler.debug_memory_mirrors);
        assert!(config.first_run.phase3_wizard_pending);
    }

    #[test]
    fn wizard_requires_all_core_acknowledgement() {
        let mut config = config_with_temp_storage();
        config.compiler.compile_thread_cap = super::available_parallelism();
        config.compiler.all_core_warning_acknowledged = false;

        let error = config
            .validate_wizard()
            .expect_err("all-core warning must be acknowledged");

        assert!(
            error
                .errors()
                .contains(&WizardValidationError::AllCoreWarningNotAcknowledged)
        );
    }

    #[test]
    fn wizard_can_complete_after_validation() {
        let mut config = config_with_temp_storage();
        config.compiler.all_core_warning_acknowledged = true;

        config
            .complete_first_launch()
            .expect("valid wizard config should complete");

        assert!(!config.first_run.phase3_wizard_pending);
        assert_eq!(config.ui.selected_screen, AppScreen::Library);
        assert!(config.first_run.wizard_completed_at_unix_secs.is_some());
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = config_with_temp_storage();
        config.ui.selected_screen = AppScreen::Settings;
        config.ui.theme_mode = ThemeMode::Light;
        config.ui.developer_mode_visible = true;
        config.compiler.all_core_warning_acknowledged = true;
        config.compiler.native_patch_chaining = true;
        config.compiler.debug_memory_mirrors = true;
        config.input.gamepad_enabled = false;
        config
            .input
            .keyboard
            .set_key_for(super::InputBinding::A, super::KeyboardKey::K);

        let serialized = toml::to_string_pretty(&config).expect("config should serialize");
        let decoded: AppConfig = toml::from_str(&serialized).expect("config should parse");

        assert_eq!(decoded, config);
    }

    #[test]
    fn old_config_without_input_section_uses_defaults() {
        let decoded: AppConfig = toml::from_str(
            r#"
            [ui]
            selected-screen = "settings"
            theme-mode = "light"
            developer-mode-visible = true
            "#,
        )
        .expect("old config should parse");

        assert_eq!(decoded.ui.selected_screen, AppScreen::Settings);
        assert!(decoded.input.gamepad_enabled);
        assert_eq!(
            decoded
                .input
                .keyboard
                .key_for(super::InputBinding::DPadLeft),
            super::KeyboardKey::A
        );
        assert_eq!(
            decoded.input.keyboard.key_for(super::InputBinding::Plus),
            super::KeyboardKey::Enter
        );
    }

    #[test]
    fn partial_input_config_uses_keyboard_defaults() {
        let decoded: AppConfig = toml::from_str(
            r#"
            [input]
            gamepad-enabled = false

            [input.keyboard]
            a = "b"
            "#,
        )
        .expect("partial input config should parse");

        assert!(!decoded.input.gamepad_enabled);
        assert_eq!(
            decoded.input.keyboard.key_for(super::InputBinding::A),
            super::KeyboardKey::B
        );
        assert_eq!(
            decoded.input.keyboard.key_for(super::InputBinding::DPadUp),
            super::KeyboardKey::W
        );
        assert_eq!(
            decoded.input.keyboard.key_for(super::InputBinding::Plus),
            super::KeyboardKey::Enter
        );
    }

    #[test]
    fn config_store_saves_and_loads() {
        let dir = tempdir().expect("temp dir should be created");
        let path = dir.path().join("config.toml");
        let store = ConfigStore::from_path(&path);
        let mut config = config_with_temp_storage();
        config.ui.selected_screen = AppScreen::Compile;

        store.save(&config).expect("config should save");
        let loaded = store.load().expect("config should load");

        assert_eq!(loaded, config);
    }

    #[test]
    fn missing_config_reports_not_found() {
        let dir = tempdir().expect("temp dir should be created");
        let store = ConfigStore::from_path(dir.path().join("missing.toml"));

        let result = store.load();

        assert!(matches!(result, Err(ConfigError::NotFound { .. })));
    }

    #[test]
    fn invalid_config_reports_parse_error() {
        let dir = tempdir().expect("temp dir should be created");
        let path = dir.path().join("invalid.toml");
        fs::write(&path, "not = [valid").expect("invalid config should be writable");
        let store = ConfigStore::from_path(&path);

        let result = store.load();

        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    fn config_with_temp_storage() -> AppConfig {
        let root = PathBuf::from("/tmp/nx86-config-test");
        let mut config = AppConfig {
            storage: StorageConfig::from_roots(root.join("data"), root.join("cache")),
            ..AppConfig::default()
        };
        config.storage.library_folders = vec![root.join("library")];
        config.compiler.compile_thread_cap = 1;
        config
    }
}
