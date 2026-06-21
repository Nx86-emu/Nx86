mod screens;
mod theme;

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
};

use eframe::egui;
use nx86_arm64_decode::decode_program;
use nx86_core::{
    config::{AppConfig, AppScreen, ConfigError, ConfigStore},
    ipc::{CancelledEvent, IpcEvent, WorkerKind, decode_event},
    storage::StorageLayout,
};
use nx86_runtime::{NativeStatus, run_synthetic_test};
use nx86_testsuite::SyntheticArm64Test;
use nx86_title_db::{TitleDatabase, TitleEntry, TitleId};
use nx86_vmm::{GuestAddress, GuestMemory, PagePermissions};
use tracing::{info, warn};

pub use nx86_core::config::ThemeMode;

pub fn run() -> eframe::Result<()> {
    let (config_store, config) = load_startup_config();
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("Nx86")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([920.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Nx86",
        native_options,
        Box::new(move |creation_context| {
            Ok(Box::new(Nx86App::new(
                creation_context,
                config,
                config_store,
            )))
        }),
    )
}

fn load_startup_config() -> (Option<ConfigStore>, AppConfig) {
    let store = match ConfigStore::for_linux_xdg() {
        Ok(store) => store,
        Err(error) => {
            warn!(%error, "using in-memory defaults because config storage is unavailable");
            return (None, AppConfig::default());
        }
    };

    let config = match store.load() {
        Ok(config) => {
            info!(path = %store.path().display(), "loaded Nx86 config");
            config
        }
        Err(ConfigError::NotFound { .. }) => {
            let config = AppConfig::default();
            if let Err(error) = store.save(&config) {
                warn!(%error, "failed to create default config");
            }
            config
        }
        Err(error) => {
            warn!(%error, "using default config after load failure");
            AppConfig::default()
        }
    };

    (Some(store), config)
}

pub struct Nx86App {
    config: AppConfig,
    config_store: Option<ConfigStore>,
    last_saved_config: AppConfig,
    last_save_error: Option<String>,
    storage_layout: Option<StorageLayout>,
    title_database: Option<TitleDatabase>,
    titles: Vec<TitleEntry>,
    service_error: Option<String>,
    library_ui: screens::LibraryUiState,
    compile_ui: screens::CompileUiState,
    test_ui: screens::TestUiState,
    worker_process: Option<WorkerProcess>,
}

impl Nx86App {
    #[must_use]
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        config: AppConfig,
        config_store: Option<ConfigStore>,
    ) -> Self {
        theme::apply_theme(&creation_context.egui_ctx, config.ui.theme_mode);

        let mut app = Self {
            last_saved_config: config.clone(),
            config,
            config_store,
            last_save_error: None,
            storage_layout: None,
            title_database: None,
            titles: Vec::new(),
            service_error: None,
            library_ui: screens::LibraryUiState::default(),
            compile_ui: screens::CompileUiState::default(),
            test_ui: screens::TestUiState::default(),
            worker_process: None,
        };
        app.initialize_services_if_ready();
        app
    }

    #[cfg(test)]
    #[must_use]
    pub fn new_for_test(config: AppConfig) -> Self {
        Self {
            last_saved_config: config.clone(),
            config,
            config_store: None,
            last_save_error: None,
            storage_layout: None,
            title_database: None,
            titles: Vec::new(),
            service_error: None,
            library_ui: screens::LibraryUiState::default(),
            compile_ui: screens::CompileUiState::default(),
            test_ui: screens::TestUiState::default(),
            worker_process: None,
        }
    }

    #[must_use]
    pub const fn selected_screen(&self) -> AppScreen {
        self.config.ui.selected_screen
    }

    pub fn set_selected_screen(&mut self, screen: AppScreen) {
        self.config.ui.selected_screen = screen;
    }

    fn config_root(&self) -> std::path::PathBuf {
        self.config_store.as_ref().map_or_else(
            || std::path::PathBuf::from("config"),
            ConfigStore::config_root,
        )
    }

    fn initialize_services_if_ready(&mut self) {
        if self.config.wizard_is_pending() {
            return;
        }

        let layout = StorageLayout::from_config(self.config_root(), &self.config.storage);
        match TitleDatabase::open(layout.clone()) {
            Ok(database) => {
                self.storage_layout = Some(layout);
                self.title_database = Some(database);
                self.service_error = None;
                self.refresh_titles();
            }
            Err(error) => {
                self.storage_layout = Some(layout);
                self.title_database = None;
                self.service_error = Some(error.to_string());
            }
        }
    }

    fn refresh_titles(&mut self) {
        let Some(database) = &self.title_database else {
            self.titles.clear();
            return;
        };

        match database.list_titles() {
            Ok(titles) => {
                self.titles = titles;
                self.service_error = None;
            }
            Err(error) => {
                self.service_error = Some(error.to_string());
            }
        }
    }

    fn create_placeholder_title(&mut self) {
        let Some(database) = &self.title_database else {
            self.library_ui.message = Some("title database is not available".to_owned());
            return;
        };

        let title_id = match TitleId::parse(&self.library_ui.new_title_id) {
            Ok(title_id) => title_id,
            Err(error) => {
                self.library_ui.message = Some(error.to_string());
                return;
            }
        };

        let display_name = self.library_ui.new_display_name.trim();
        if display_name.is_empty() {
            self.library_ui.message = Some("display name is required".to_owned());
            return;
        }

        match database.create_placeholder(title_id, display_name) {
            Ok(_) => {
                self.library_ui.new_title_id.clear();
                self.library_ui.new_display_name.clear();
                self.library_ui.message = Some("placeholder title created".to_owned());
                self.refresh_titles();
            }
            Err(error) => {
                self.library_ui.message = Some(error.to_string());
            }
        }
    }

    fn complete_wizard(&mut self) {
        match self.config.complete_first_launch() {
            Ok(()) => {
                self.persist_now();
                self.initialize_services_if_ready();
            }
            Err(error) => {
                self.service_error = Some(error.to_string());
            }
        }
    }

    fn persist_now(&mut self) {
        let Some(store) = &self.config_store else {
            self.last_saved_config = self.config.clone();
            return;
        };

        match store.save(&self.config) {
            Ok(()) => {
                self.last_saved_config = self.config.clone();
                self.last_save_error = None;
            }
            Err(error) => {
                let message = error.to_string();
                warn!(%error, "failed to persist Nx86 settings");
                self.last_save_error = Some(message);
            }
        }
    }

    fn persist_if_changed(&mut self) {
        if self.config != self.last_saved_config {
            self.persist_now();
        }
    }

    fn draw_navigation(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading("Nx86");
        ui.label("Continuous Dynamic Compilation");
        ui.add_space(16.0);

        for screen in AppScreen::ALL {
            let selected = self.config.ui.selected_screen == screen;
            let response = ui.add_enabled(
                !self.config.wizard_is_pending(),
                egui::Button::new(screen.label()).selected(selected),
            );

            if response.clicked() {
                self.config.ui.selected_screen = screen;
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label("Linux x86_64-v4");
            ui.label("Vulkan: ash boundary");
        });
    }

    fn draw_content(&mut self, ui: &mut egui::Ui) {
        self.poll_worker();

        if self.config.wizard_is_pending() {
            match screens::first_launch_wizard(ui, &mut self.config, self.service_error.as_deref())
            {
                screens::WizardAction::None => {}
                screens::WizardAction::Complete => self.complete_wizard(),
            }
            return;
        }

        match self.config.ui.selected_screen {
            AppScreen::Library => match screens::library(
                ui,
                &self.titles,
                &mut self.library_ui,
                self.service_error.as_deref(),
            ) {
                screens::LibraryAction::None => {}
                screens::LibraryAction::CreatePlaceholder => self.create_placeholder_title(),
                screens::LibraryAction::Refresh => self.refresh_titles(),
            },
            AppScreen::Compile => match screens::compile(ui, &mut self.compile_ui) {
                screens::CompileAction::None => {}
                screens::CompileAction::StartCompilerSmoke => {
                    self.start_worker(WorkerKind::CompilerSmoke);
                }
                screens::CompileAction::StartRuntimeSmoke => {
                    self.start_worker(WorkerKind::RuntimeSmoke);
                }
                screens::CompileAction::Cancel => self.cancel_worker(),
            },
            AppScreen::Tests => match screens::tests(ui, &mut self.test_ui) {
                screens::TestAction::None => {}
                screens::TestAction::PickFile => self.pick_synthetic_test_file(),
                screens::TestAction::LoadPath => self.load_synthetic_test_from_state(),
            },
            AppScreen::Settings => {
                if screens::settings(ui, &mut self.config, self.storage_layout.as_ref()) {
                    theme::apply_theme(ui.ctx(), self.config.ui.theme_mode);
                }
            }
        }
    }

    fn start_worker(&mut self, kind: WorkerKind) {
        if self.worker_process.is_some() {
            self.compile_ui.status = "worker already running".to_owned();
            return;
        }

        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                self.compile_ui.status = error.to_string();
                return;
            }
        };

        let mut child = match Command::new(executable)
            .arg("--worker")
            .arg(kind.label())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                self.compile_ui.status = error.to_string();
                return;
            }
        };

        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            self.compile_ui.status = "worker stdout was unavailable".to_owned();
            return;
        };

        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let event = match line {
                    Ok(line) => decode_event(&line).unwrap_or_else(|error| {
                        IpcEvent::Log(nx86_core::ipc::LogEvent {
                            level: nx86_core::ipc::LogLevel::Warn,
                            message: error.to_string(),
                        })
                    }),
                    Err(error) => IpcEvent::Log(nx86_core::ipc::LogEvent {
                        level: nx86_core::ipc::LogLevel::Warn,
                        message: error.to_string(),
                    }),
                };

                if sender.send(event).is_err() {
                    break;
                }
            }
        });

        self.compile_ui.status = format!("{} running", kind.label());
        self.compile_ui.running = true;
        self.worker_process = Some(WorkerProcess {
            child,
            receiver,
            reader: Some(reader),
        });
    }

    fn poll_worker(&mut self) {
        let Some(mut worker) = self.worker_process.take() else {
            return;
        };

        self.drain_worker_events(&worker.receiver);

        match worker.child.try_wait() {
            Ok(Some(status)) => {
                if worker.join_reader().is_err() {
                    self.compile_ui
                        .apply_event(IpcEvent::Log(nx86_core::ipc::LogEvent {
                            level: nx86_core::ipc::LogLevel::Warn,
                            message: "worker stdout reader thread panicked".to_owned(),
                        }));
                }
                self.drain_worker_events(&worker.receiver);
                self.compile_ui.running = false;
                if self.compile_ui.status.contains("running") {
                    self.compile_ui.status = format!("worker exited with {status}");
                }
            }
            Ok(None) => {
                self.worker_process = Some(worker);
            }
            Err(error) => {
                self.compile_ui.running = false;
                self.compile_ui.status = error.to_string();
            }
        }
    }

    fn cancel_worker(&mut self) {
        let Some(mut worker) = self.worker_process.take() else {
            self.compile_ui.status = "no worker is running".to_owned();
            return;
        };

        let kill_result = worker.child.kill();
        let wait_result = worker.child.wait();
        let reader_result = worker.join_reader();
        self.drain_worker_events(&worker.receiver);

        if kill_result.is_ok() && wait_result.is_ok() && reader_result.is_ok() {
            self.compile_ui
                .apply_event(IpcEvent::Cancelled(CancelledEvent {
                    job_id: "gui-worker".to_owned(),
                    reason: "cancelled by user".to_owned(),
                }));
        } else if let Err(error) = kill_result {
            self.compile_ui.status = error.to_string();
        } else if let Err(error) = wait_result {
            self.compile_ui.status = error.to_string();
        } else {
            self.compile_ui.status = "worker stdout reader thread panicked".to_owned();
        }
    }

    fn drain_worker_events(&mut self, receiver: &Receiver<IpcEvent>) {
        while let Ok(event) = receiver.try_recv() {
            self.compile_ui.apply_event(event);
        }
    }

    fn pick_synthetic_test_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Nx86 synthetic ARM64 test", &["toml"])
            .pick_file()
        else {
            return;
        };
        self.test_ui.path = path.display().to_string();
        self.load_synthetic_test_from_state();
    }

    fn load_synthetic_test_from_state(&mut self) {
        match SyntheticArm64Test::load(&self.test_ui.path) {
            Ok(test) => {
                self.analyze_synthetic_test(&test);
                self.test_ui.loaded = Some(test);
                self.test_ui.message = None;
            }
            Err(error) => {
                self.test_ui.loaded = None;
                self.test_ui.decoded.clear();
                self.test_ui.run_status = None;
                self.test_ui.register_diffs.clear();
                self.test_ui.memory_dumps.clear();
                self.test_ui.framebuffer = None;
                self.test_ui.framebuffer_texture = None;
                self.test_ui.nxir_status = None;
                self.test_ui.nxir_dump = None;
                self.test_ui.native_status = None;
                self.test_ui.native_dump = None;
                self.test_ui.message = Some(error.to_string());
            }
        }
    }

    fn analyze_synthetic_test(&mut self, test: &SyntheticArm64Test) {
        self.test_ui.decoded.clear();
        self.test_ui.run_status = None;
        self.test_ui.register_diffs.clear();
        self.test_ui.memory_dumps.clear();
        self.test_ui.framebuffer = None;
        self.test_ui.framebuffer_texture = None;
        self.test_ui.nxir_status = None;
        self.test_ui.nxir_dump = None;
        self.test_ui.native_status = None;
        self.test_ui.native_dump = None;

        match test.entry_point() {
            Ok(entry) => match decode_program(&test.program.bytes, entry) {
                Ok(instructions) => {
                    self.test_ui.decoded = instructions
                        .iter()
                        .map(|instruction| {
                            format!(
                                "{:#010x}: {:08x}    {}",
                                instruction.address, instruction.word, instruction.disassembly
                            )
                        })
                        .collect();
                }
                Err(error) => {
                    self.test_ui.decoded.push(format!("decode failed: {error}"));
                }
            },
            Err(error) => {
                self.test_ui
                    .decoded
                    .push(format!("entry point failed: {error}"));
            }
        }

        match run_synthetic_test(test) {
            Ok(result) => {
                self.test_ui.run_status = Some(format!(
                    "halted={} pc={:#x} trace={} step(s)",
                    result.interpreter.final_state.halted(),
                    result.interpreter.final_state.pc(),
                    result.interpreter.trace.len()
                ));
                self.test_ui.register_diffs = result
                    .register_diffs
                    .iter()
                    .map(|diff| {
                        format!(
                            "{} expected {}, actual {}",
                            diff.register, diff.expected, diff.actual
                        )
                    })
                    .collect();
                self.test_ui.framebuffer = result
                    .framebuffer
                    .map(|framebuffer| (framebuffer.width, framebuffer.height, framebuffer.bytes));
                self.test_ui.nxir_status = Some(match (&result.nxir.error, result.nxir.agrees) {
                    (Some(error), _) => format!("unavailable: {error}"),
                    (None, true) => "matches interpreter".to_owned(),
                    (None, false) => "disagrees with interpreter".to_owned(),
                });
                self.test_ui.nxir_dump =
                    (!result.nxir.dump.is_empty()).then(|| result.nxir.dump.clone());
                self.test_ui.native_status = Some(native_status_line(
                    result.native.status,
                    result.native.error.as_deref(),
                ));
                self.test_ui.native_dump =
                    (!result.native.dump.is_empty()).then(|| result.native.dump.clone());
            }
            Err(error) => {
                self.test_ui.run_status = Some(format!("interpreter failed: {error}"));
            }
        }

        for range in &test.expected.memory {
            match memory_dump_summary(range) {
                Ok(summary) => self.test_ui.memory_dumps.push(summary),
                Err(error) => self.test_ui.memory_dumps.push(error),
            }
        }
    }
}

impl eframe::App for Nx86App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("nx86-navigation")
            .resizable(false)
            .exact_size(220.0)
            .show_inside(ui, |ui| self.draw_navigation(ui));

        egui::CentralPanel::default().show_inside(ui, |ui| self.draw_content(ui));

        self.persist_if_changed();
    }
}

struct WorkerProcess {
    child: Child,
    receiver: Receiver<IpcEvent>,
    reader: Option<JoinHandle<()>>,
}

impl WorkerProcess {
    fn join_reader(&mut self) -> Result<(), ()> {
        if let Some(reader) = self.reader.take() {
            reader.join().map_err(|_| ())?;
        }
        Ok(())
    }
}

fn memory_dump_summary(range: &nx86_testsuite::ExpectedMemoryRange) -> Result<String, String> {
    let address = range.address_u64().map_err(|error| error.to_string())?;
    if range.bytes.is_empty() {
        return Ok(format!("{address:#x}: <empty>"));
    }
    let mut memory = GuestMemory::new_logical();
    let start_page = GuestAddress(address).page_base();
    let end = address
        .checked_add(range.bytes.len() as u64)
        .ok_or_else(|| "memory range overflows u64".to_owned())?;
    let mut page = start_page;
    while page <= GuestAddress(end.saturating_sub(1)).page_base() {
        memory
            .map_page(GuestAddress(page), PagePermissions::READ_WRITE)
            .map_err(|error| error.to_string())?;
        page += nx86_vmm::PAGE_SIZE;
    }
    memory
        .write(GuestAddress(address), &range.bytes)
        .map_err(|error| error.to_string())?;
    let dump = memory
        .debug_dump(GuestAddress(address), range.bytes.len())
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "{:#x}: {}",
        dump.start.0,
        dump.bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

fn native_status_line(status: NativeStatus, error: Option<&str>) -> String {
    match (status, error) {
        (NativeStatus::MatchesInterpreter, _) => "matches interpreter".to_owned(),
        (NativeStatus::DisagreesWithInterpreter, _) => "disagrees with interpreter".to_owned(),
        (NativeStatus::Unsupported, Some(error)) => format!("unsupported: {error}"),
        (NativeStatus::Unsupported, None) => "unsupported".to_owned(),
        (NativeStatus::Unavailable, Some(error)) => format!("unavailable: {error}"),
        (NativeStatus::Unavailable, None) => "unavailable".to_owned(),
        (NativeStatus::Error, Some(error)) => format!("error: {error}"),
        (NativeStatus::Error, None) => "error".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use nx86_core::config::{AppConfig, AppScreen};
    use nx86_testsuite::SyntheticArm64Test;

    use super::Nx86App;

    #[test]
    fn navigation_state_changes() {
        let mut app = Nx86App::new_for_test(AppConfig::default());

        app.set_selected_screen(AppScreen::Compile);

        assert_eq!(app.selected_screen(), AppScreen::Compile);
    }

    #[test]
    fn synthetic_test_analysis_updates_decode_run_and_memory_state() {
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "gui add"

            [program]
            arm64-hex = "20 00 80 D2 01 08 00 91 01 00 00 D4"

            [expected.registers]
            x1 = "0x3"
            halted = "true"

            [[expected.memory]]
            address = "0x1000"
            bytes-hex = "AA BB"
            "#,
        )
        .expect("test should parse");
        let mut app = Nx86App::new_for_test(AppConfig::default());

        app.analyze_synthetic_test(&test);

        assert_eq!(app.test_ui.decoded.len(), 3);
        assert!(app.test_ui.decoded[0].contains("mov x0"));
        assert!(app.test_ui.run_status.as_deref().is_some_and(|status| {
            status.contains("halted=true") && status.contains("trace=3")
        }));
        assert!(app.test_ui.register_diffs.is_empty());
        assert_eq!(app.test_ui.memory_dumps, vec!["0x1000: aa bb"]);
        assert!(app.test_ui.native_status.as_deref().is_some_and(|status| {
            status.contains("unavailable") || status.contains("matches interpreter")
        }));
        assert!(app.test_ui.native_dump.is_some());
    }
}
