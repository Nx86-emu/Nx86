use egui::{Color32, Context, FontFamily, FontId, Style, TextStyle, Visuals};
use nx86_core::config::ThemeMode;

pub fn apply_theme(context: &Context, mode: ThemeMode) {
    let mut style = Style::default();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.visuals = match mode {
        ThemeMode::Dark => dark_visuals(),
        ThemeMode::Light => light_visuals(),
    };

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(24.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ]
    .into();

    context.set_global_style(style);
}

fn dark_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(24, 25, 27);
    visuals.window_fill = Color32::from_rgb(30, 32, 35);
    visuals.faint_bg_color = Color32::from_rgb(36, 38, 41);
    visuals.extreme_bg_color = Color32::from_rgb(15, 16, 18);
    visuals.hyperlink_color = Color32::from_rgb(95, 180, 210);
    visuals.selection.bg_fill = Color32::from_rgb(64, 152, 124);
    visuals.widgets.active.bg_fill = Color32::from_rgb(64, 152, 124);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 80, 72);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(38, 40, 43);
    visuals
}

fn light_visuals() -> Visuals {
    let mut visuals = Visuals::light();
    visuals.panel_fill = Color32::from_rgb(242, 243, 241);
    visuals.window_fill = Color32::from_rgb(250, 250, 248);
    visuals.faint_bg_color = Color32::from_rgb(232, 235, 232);
    visuals.hyperlink_color = Color32::from_rgb(32, 104, 136);
    visuals.selection.bg_fill = Color32::from_rgb(76, 145, 116);
    visuals.widgets.active.bg_fill = Color32::from_rgb(76, 145, 116);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(214, 230, 222);
    visuals
}
