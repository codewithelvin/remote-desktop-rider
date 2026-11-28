use crate::widgets;
use eframe::egui::{self, Align, Layout, RichText, Ui};
use rider_core::trf;
use std::time::{Duration, Instant};

pub const CANCEL: &str = "restart-cancel";

#[derive(Debug, PartialEq)]
pub enum RestartEvent {
    Warning(u64),
    Cancelled,
}

pub fn parse(v: &serde_json::Value) -> Option<RestartEvent> {
    match v.get("event")?.as_str()? {
        "restart" => Some(RestartEvent::Warning(v.get("seconds").and_then(serde_json::Value::as_u64)?)),
        "restart_cancelled" => Some(RestartEvent::Cancelled),
        _ => None,
    }
}

pub struct RestartWarning {
    until: Instant,
    total: f32,
}

impl RestartWarning {
    pub fn new(seconds: u64) -> Self {
        Self { until: Instant::now() + Duration::from_secs(seconds), total: seconds.max(1) as f32 }
    }

    fn left(&self) -> Duration {
        self.until.saturating_duration_since(Instant::now())
    }
}

pub fn banner(ui: &mut Ui, warning: &RestartWarning, device: &str) -> bool {
    let left = warning.left();
    let counting = !left.is_zero();
    if counting {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    let dark = ui.visuals().dark_mode;
    let fill = if dark { egui::Color32::from_rgb(64, 30, 28) } else { egui::Color32::from_rgb(255, 228, 224) };
    let mut cancel = false;
    egui::Panel::top("restart-warning")
        .resizable(false)
        .show_separator_line(true)
        .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(14, 9)))
        .show(ui, |ui| {
            let text = trf!("\"{device}\" is restarting this computer", "\"{device}\" bu kompüteri yenidən başladır");
            ui.add(egui::Label::new(widgets::title(&text, 13.5)).wrap());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let status = if counting {
                    let secs = left.as_secs() + 1;
                    trf!(
                        "Save your work. Restarts in {secs} s",
                        "İşinizi yadda saxlayın. {secs} san. sonra yenidən başlayacaq"
                    )
                } else {
                    trf!("Restarting…", "Yenidən başlayır…")
                };
                ui.label(RichText::new(status).weak());
                if counting {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        cancel = widgets::danger(ui, rider_core::i18n::tr("Cancel restart", "Ləğv et")).clicked();
                    });
                }
            });
            ui.add_space(6.0);
            let part = (left.as_secs_f32() / warning.total).clamp(0.0, 1.0);
            let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, 1.5, ui.visuals().widgets.noninteractive.bg_stroke.color);
            let done = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * part, bar.height()));
            ui.painter().rect_filled(done, 1.5, widgets::AMBER);
        });
    cancel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(line: &str) -> Option<RestartEvent> {
        parse(&serde_json::from_str(line).unwrap())
    }

    #[test]
    fn restart_events() {
        assert_eq!(ev(r#"{"event":"restart","seconds":15}"#), Some(RestartEvent::Warning(15)));
        assert_eq!(ev(r#"{"event":"restart_cancelled"}"#), Some(RestartEvent::Cancelled));
        assert_eq!(ev(r#"{"event":"restart"}"#), None);
        assert_eq!(ev(r#"{"event":"chat","text":"x"}"#), None);
        assert!(!RestartWarning::new(15).left().is_zero());
        assert!(RestartWarning::new(0).left().is_zero());
    }
}
