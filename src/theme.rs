//! Palette e geometria condivise: un'unica scala di spaziatura per tutta la UI.
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

pub const BACKGROUND: Color32 = Color32::from_rgb(13, 21, 47);
pub const PANEL: Color32 = Color32::from_rgb(17, 27, 58);
pub const SURFACE: Color32 = Color32::from_rgb(24, 36, 71);
pub const BORDER: Color32 = Color32::from_rgb(47, 57, 91);
pub const ACCENT: Color32 = Color32::from_rgb(252, 100, 147);
pub const BLUE: Color32 = Color32::from_rgb(96, 174, 239);
pub const TEXT: Color32 = Color32::from_rgb(246, 239, 248);
pub const MUTED: Color32 = Color32::from_rgb(165, 170, 199);
pub const WARNING: Color32 = Color32::from_rgb(255, 197, 112);
pub const DANGER: Color32 = Color32::from_rgb(255, 132, 159);
pub const GAP: f32 = 8.0;
pub const PADDING: i8 = 16;

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.spacing.item_spacing = Vec2::splat(GAP);
    style.spacing.button_padding = Vec2::new(14.0, 9.0);
    style.spacing.interact_size.y = 34.0;
    style.spacing.slider_width = 210.0;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.faint_bg_color = PANEL;
    style.visuals.window_corner_radius = egui::CornerRadius::same(12);
    style.visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.selection.bg_fill = Color32::from_rgb(42, 91, 91);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.slider_trailing_fill = true;
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        widget.corner_radius = egui::CornerRadius::same(8);
        widget.bg_stroke = Stroke::new(1.0_f32, BORDER);
    }
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 61, 78);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(37, 79, 79);
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(23.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    ctx.set_style(style);
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(PADDING))
}

pub fn card(ui: &mut egui::Ui, number: &str, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(number).small().strong().color(ACCENT));
                ui.label(RichText::new(title).strong());
            });
            ui.add_space(GAP);
            body(ui);
        });
    ui.add_space(GAP);
}

pub fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).strong().color(BACKGROUND))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(190.0, 40.0))
}

/// Logo vettoriale, senza font di icone o asset esterni.
pub fn logo(ui: &mut egui::Ui, texture: &egui::TextureHandle, size: f32) {
    ui.add(egui::Image::new(texture).fit_to_exact_size(Vec2::splat(size)));
}
