use std::path::PathBuf;

use eframe::egui;
use nx86_core::{
    config::{AppConfig, ThemeMode, available_parallelism},
    ipc::{CompileProgress, IpcEvent, LogLevel},
    storage::StorageLayout,
};
use nx86_testsuite::SyntheticArm64Test;
use nx86_title_db::TitleEntry;

#[derive(Default)]
pub struct LibraryUiState {
    pub new_title_id: String,
    pub new_display_name: String,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryAction {
    None,
    CreatePlaceholder,
    Refresh,
}

#[derive(Default)]
pub struct CompileUiState {
    pub running: bool,
    pub status: String,
    pub progress: Option<CompileProgress>,
    pub logs: Vec<String>,
}

impl CompileUiState {
    pub fn apply_event(&mut self, event: IpcEvent) {
        match event {
            IpcEvent::Progress(progress) => {
                self.status = format!("{} {:.0}%", progress.phase, progress.percent);
                self.progress = Some(progress);
            }
            IpcEvent::Cancelled(cancelled) => {
                self.running = false;
                self.status = cancelled.reason;
            }
            IpcEvent::Log(log) => {
                let level = match log.level {
                    LogLevel::Info => "info",
                    LogLevel::Warn => "warn",
                    LogLevel::Error => "error",
                };
                self.logs.push(format!("[{level}] {}", log.message));
            }
            IpcEvent::Completed(completed) => {
                self.running = false;
                self.status = completed.message;
            }
        }

        if self.logs.len() > 12 {
            let excess = self.logs.len() - 12;
            self.logs.drain(0..excess);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileAction {
    None,
    StartCompilerSmoke,
    StartRuntimeSmoke,
    Cancel,
}

#[derive(Default)]
pub struct TestUiState {
    pub path: String,
    pub loaded: Option<SyntheticArm64Test>,
    pub message: Option<String>,
    pub decoded: Vec<String>,
    pub run_status: Option<String>,
    pub register_diffs: Vec<String>,
    pub memory_dumps: Vec<String>,
    /// Rendered framebuffer pixels `(width, height, rgba8 bytes)` from the last
    /// run, if the loaded test declared a `[framebuffer]`.
    pub framebuffer: Option<(u32, u32, Vec<u8>)>,
    /// Cached GPU texture for [`Self::framebuffer`]; rebuilt when the pixels
    /// change (cleared by the analysis step).
    pub framebuffer_texture: Option<egui::TextureHandle>,
    /// Whether NxIR evaluation agreed with the interpreter, as a status line.
    pub nxir_status: Option<String>,
    /// NxIR text dump from the last run, if lifting succeeded.
    pub nxir_dump: Option<String>,
    /// Phase 18 native x86_64 execution status, as a compact status line.
    pub native_status: Option<String>,
    /// Native assembler dump from the last run, if lowering succeeded.
    pub native_dump: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestAction {
    None,
    PickFile,
    LoadPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardAction {
    None,
    Complete,
}

pub fn first_launch_wizard(
    ui: &mut egui::Ui,
    config: &mut AppConfig,
    message: Option<&str>,
) -> WizardAction {
    screen_header(ui, "First-Launch Wizard", "Phase 3");
    ui.label("Configure the required folders and compile behavior before entering Library.");
    ui.add_space(12.0);

    folder_rows(ui, config);

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        ui.label("CPU target");
        ui.monospace(config.compiler.cpu_target.label());
    });

    let available = available_parallelism();
    ui.horizontal(|ui| {
        ui.label("Compile thread cap");
        ui.add(
            egui::Slider::new(
                &mut config.compiler.compile_thread_cap,
                1..=available.saturating_mul(2),
            )
            .text("threads"),
        );
    });

    if config.compiler.compile_thread_cap >= available {
        ui.checkbox(
            &mut config.compiler.all_core_warning_acknowledged,
            "I understand all-core compilation can make the desktop less responsive",
        );
    }

    ui.checkbox(
        &mut config.profile_sharing.enabled,
        "Allow profile sharing prompts",
    );

    ui.horizontal(|ui| {
        ui.label("Graphics backend");
        ui.monospace(config.graphics.backend.label());
    });

    if let Some(message) = message {
        ui.add_space(10.0);
        ui.label(message);
    }

    ui.add_space(16.0);
    if ui.button("Save and Open Library").clicked() {
        WizardAction::Complete
    } else {
        WizardAction::None
    }
}

pub fn library(
    ui: &mut egui::Ui,
    titles: &[TitleEntry],
    state: &mut LibraryUiState,
    service_error: Option<&str>,
) -> LibraryAction {
    screen_header(ui, "Library", "Placeholder titles only");
    if let Some(error) = service_error {
        ui.label(error);
    }

    ui.horizontal(|ui| {
        ui.label("Title ID");
        ui.text_edit_singleline(&mut state.new_title_id);
        ui.label("Display name");
        ui.text_edit_singleline(&mut state.new_display_name);
    });

    let mut action = LibraryAction::None;
    ui.horizontal(|ui| {
        if ui.button("Create Placeholder").clicked() {
            action = LibraryAction::CreatePlaceholder;
        }
        if ui.button("Refresh").clicked() {
            action = LibraryAction::Refresh;
        }
    });

    if let Some(message) = &state.message {
        ui.label(message);
    }

    ui.add_space(12.0);
    egui::Grid::new("library-grid")
        .num_columns(5)
        .min_col_width(130.0)
        .spacing([20.0, 10.0])
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Title");
            ui.strong("Title ID");
            ui.strong("Source");
            ui.strong("Native Coverage");
            ui.strong("Cache");
            ui.end_row();

            if titles.is_empty() {
                ui.label("Library is empty");
                ui.label("-");
                ui.label("-");
                ui.label("0.00%");
                ui.label("No cache");
                ui.end_row();
            }

            for title in titles {
                ui.label(&title.display_name);
                ui.monospace(title.title_id.as_str());
                ui.label(title.source_kind.as_str());
                ui.label("0.00%");
                ui.label("No cache");
                ui.end_row();
            }
        });

    action
}

pub fn compile(ui: &mut egui::Ui, state: &mut CompileUiState) -> CompileAction {
    screen_header(ui, "Compile", &state.status);
    let mut action = CompileAction::None;

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !state.running,
                egui::Button::new("Launch Compiler Smoke Worker"),
            )
            .clicked()
        {
            action = CompileAction::StartCompilerSmoke;
        }
        if ui
            .add_enabled(
                !state.running,
                egui::Button::new("Launch Runtime Smoke Worker"),
            )
            .clicked()
        {
            action = CompileAction::StartRuntimeSmoke;
        }
        if ui
            .add_enabled(state.running, egui::Button::new("Cancel"))
            .clicked()
        {
            action = CompileAction::Cancel;
        }
    });

    ui.add_space(12.0);
    let progress = state.progress.clone().unwrap_or(CompileProgress {
        title_id: None,
        phase: "idle".to_owned(),
        percent: 0.0,
        current_module: None,
        functions_discovered: 0,
        functions_compiled: 0,
        native_coverage_estimate: 0.0,
        cache_size_bytes: 0,
    });

    ui.add(
        egui::ProgressBar::new(progress.percent / 100.0)
            .text(format!("{} {:.0}%", progress.phase, progress.percent)),
    );

    ui.columns(3, |columns| {
        metric(
            &mut columns[0],
            "Functions discovered",
            &progress.functions_discovered.to_string(),
        );
        metric(
            &mut columns[1],
            "Functions compiled",
            &progress.functions_compiled.to_string(),
        );
        metric(
            &mut columns[2],
            "Native Coverage",
            &format!("{:.2}%", progress.native_coverage_estimate),
        );
    });

    ui.add_space(18.0);
    ui.separator();
    ui.add_space(12.0);
    ui.strong("Compact logs");
    for log in &state.logs {
        ui.monospace(log);
    }

    action
}

pub fn tests(ui: &mut egui::Ui, state: &mut TestUiState) -> TestAction {
    screen_header(ui, "Synthetic ARM64 Tests", "Phases 6-19");
    let mut action = TestAction::None;

    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.path);
        if ui.button("Open").clicked() {
            action = TestAction::PickFile;
        }
        if ui.button("Load").clicked() {
            action = TestAction::LoadPath;
        }
    });

    if let Some(message) = &state.message {
        ui.label(message);
    }

    if let Some(test) = &state.loaded {
        ui.add_space(12.0);
        ui.heading(&test.metadata.name);
        if !test.metadata.description.is_empty() {
            ui.label(&test.metadata.description);
        }
        ui.monospace(format!("ARM64 bytes: {} byte(s)", test.program.bytes.len()));
        ui.monospace(format!("Entry point: {}", test.metadata.entry_point));

        if let Some(status) = &state.run_status {
            ui.add_space(10.0);
            ui.strong("Interpreter");
            ui.monospace(status);
        }

        if !state.decoded.is_empty() {
            ui.add_space(10.0);
            ui.strong("Decoded instructions");
            for instruction in &state.decoded {
                ui.monospace(instruction);
            }
        }

        if !state.register_diffs.is_empty() {
            ui.add_space(10.0);
            ui.strong("Register mismatches");
            for diff in &state.register_diffs {
                ui.monospace(diff);
            }
        }

        ui.add_space(10.0);
        ui.strong("Expected registers");
        egui::Grid::new("expected-registers")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (register, value) in &test.expected.registers {
                    ui.monospace(register);
                    ui.monospace(value);
                    ui.end_row();
                }
            });

        if !state.memory_dumps.is_empty() {
            ui.add_space(10.0);
            ui.strong("VMM memory dumps");
            for dump in &state.memory_dumps {
                ui.monospace(dump);
            }
        }

        if let Some((width, height, bytes)) = &state.framebuffer {
            ui.add_space(10.0);
            ui.strong("Framebuffer");
            ui.monospace(format!("{width}x{height} RGBA8"));
            if state.framebuffer_texture.is_none() && !bytes.is_empty() {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [*width as usize, *height as usize],
                    bytes,
                );
                state.framebuffer_texture = Some(ui.ctx().load_texture(
                    "nx86-framebuffer",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
            if let Some(texture) = &state.framebuffer_texture {
                let scale = 24.0;
                let size = egui::vec2(*width as f32 * scale, *height as f32 * scale);
                ui.image(egui::load::SizedTexture::new(texture.id(), size));
            }
        }

        if let Some(status) = &state.nxir_status {
            ui.add_space(10.0);
            ui.strong("NxIR");
            ui.monospace(status);
            if let Some(dump) = &state.nxir_dump {
                egui::ScrollArea::vertical()
                    .id_salt("nxir-dump")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.monospace(dump);
                    });
            }
        }

        if let Some(status) = &state.native_status {
            ui.add_space(10.0);
            ui.strong("Native x86_64");
            ui.monospace(status);
            if let Some(dump) = &state.native_dump {
                egui::ScrollArea::vertical()
                    .id_salt("native-dump")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.monospace(dump);
                    });
            }
        }

        ui.add_space(10.0);
        ui.strong("Expected memory");
        egui::Grid::new("expected-memory")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Address");
                ui.strong("Bytes");
                ui.strong("Decoded length");
                ui.end_row();
                for range in &test.expected.memory {
                    ui.monospace(&range.address);
                    ui.monospace(&range.bytes_hex);
                    ui.label(range.bytes.len().to_string());
                    ui.end_row();
                }
            });
    }

    action
}

pub fn settings(ui: &mut egui::Ui, config: &mut AppConfig, layout: Option<&StorageLayout>) -> bool {
    screen_header(ui, "Settings", "Phase 10");
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("Theme");
        let before = config.ui.theme_mode;
        egui::ComboBox::from_id_salt("theme-mode")
            .selected_text(config.ui.theme_mode.label())
            .show_ui(ui, |ui| {
                for mode in ThemeMode::ALL {
                    ui.selectable_value(&mut config.ui.theme_mode, mode, mode.label());
                }
            });
        changed |= config.ui.theme_mode != before;
    });

    changed |= ui
        .checkbox(&mut config.ui.developer_mode_visible, "Developer tools")
        .changed();

    ui.add_space(18.0);
    egui::Grid::new("settings-grid")
        .num_columns(2)
        .min_col_width(210.0)
        .spacing([18.0, 8.0])
        .show(ui, |ui| {
            ui.label("Target OS");
            ui.monospace(&config.prototype.target_os);
            ui.end_row();

            ui.label("Target CPU");
            ui.monospace(config.compiler.cpu_target.label());
            ui.end_row();

            ui.label("Graphics backend");
            ui.monospace(config.graphics.backend.label());
            ui.end_row();

            ui.label("Compile thread cap");
            ui.monospace(config.compiler.compile_thread_cap.to_string());
            ui.end_row();

            ui.label("Profile sharing prompts");
            ui.monospace(if config.profile_sharing.enabled {
                "enabled"
            } else {
                "disabled"
            });
            ui.end_row();

            ui.label("First-launch wizard");
            ui.monospace(if config.first_run.phase3_wizard_pending {
                "pending"
            } else {
                "complete"
            });
            ui.end_row();
        });

    if let Some(layout) = layout {
        ui.add_space(18.0);
        ui.strong("Storage");
        egui::Grid::new("storage-grid")
            .num_columns(2)
            .min_col_width(210.0)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                ui.label("Titles");
                ui.monospace(layout.titles_dir.display().to_string());
                ui.end_row();
                ui.label("Database");
                ui.monospace(layout.database_path().display().to_string());
                ui.end_row();
                ui.label("Shared profiles");
                ui.monospace(layout.shared_profiles_dir.display().to_string());
                ui.end_row();
                ui.label("Global cache");
                ui.monospace(layout.global_cache_dir.display().to_string());
                ui.end_row();
            });
    }

    changed
}

fn folder_rows(ui: &mut egui::Ui, config: &mut AppConfig) {
    ui.strong("Folders");
    for index in 0..config.storage.library_folders.len() {
        ui.horizontal(|ui| {
            ui.label(format!("Library {}", index + 1));
            edit_path(ui, &mut config.storage.library_folders[index]);
            if ui.button("Browse").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                config.storage.library_folders[index] = path;
            }
        });
    }

    ui.horizontal(|ui| {
        if ui.button("Add Library Folder").clicked() {
            config.storage.library_folders.push(PathBuf::new());
        }
        if config.storage.library_folders.len() > 1 && ui.button("Remove Last").clicked() {
            config.storage.library_folders.pop();
        }
    });

    ui.horizontal(|ui| {
        ui.label("Cache");
        edit_path(ui, &mut config.storage.cache_folder);
        if ui.button("Browse").clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            config.storage.cache_folder = path;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Profiles");
        edit_path(ui, &mut config.storage.profile_folder);
        if ui.button("Browse").clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            config.storage.profile_folder = path;
        }
    });
}

fn edit_path(ui: &mut egui::Ui, path: &mut PathBuf) {
    let mut text = path.display().to_string();
    if ui.text_edit_singleline(&mut text).changed() {
        *path = PathBuf::from(text);
    }
}

fn screen_header(ui: &mut egui::Ui, title: &str, status: &str) {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.add_space(12.0);
        ui.label(status);
    });
    ui.add_space(12.0);
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(label);
        ui.heading(value);
    });
}
