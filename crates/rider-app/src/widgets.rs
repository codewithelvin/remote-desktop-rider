//! Small shared UI pieces and the worker → UI message channel.

use eframe::egui::{self, Color32, RichText, Sense, Ui, vec2};
use std::sync::mpsc;

/// Messages from background work to the UI thread; sending wakes the UI.
pub struct Bus<M> {
    tx: mpsc::Sender<M>,
    rx: mpsc::Receiver<M>,
    ctx: egui::Context,
}

impl<M> Bus<M> {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, ctx: ctx.clone() }
    }

    pub fn tx(&self) -> Tx<M> {
        Tx { tx: self.tx.clone(), ctx: self.ctx.clone() }
    }

    pub fn drain(&self) -> mpsc::TryIter<'_, M> {
        self.rx.try_iter()
    }
}

pub struct Tx<M> {
    tx: mpsc::Sender<M>,
    ctx: egui::Context,
}

impl<M> Clone for Tx<M> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), ctx: self.ctx.clone() }
    }
}

impl<M> Tx<M> {
    pub fn send(&self, msg: M) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

pub const GREEN: Color32 = Color32::from_rgb(46, 160, 67);
pub const GREY: Color32 = Color32::from_rgb(140, 140, 140);

/// A small status dot; `None` draws a hollow ring (unknown).
pub fn dot(ui: &mut Ui, color: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    match color {
        Some(c) => _ = ui.painter().circle_filled(rect.center(), 5.0, c),
        None => _ = ui.painter().circle_stroke(rect.center(), 4.5, (1.5, GREY)),
    }
}

pub fn error(ui: &mut Ui, text: &str) {
    let color = ui.visuals().error_fg_color;
    ui.label(RichText::new(text).color(color));
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).weak());
}

pub fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong().size(16.0));
    ui.add_space(2.0);
}

/// "5 min ago" style age of a Unix timestamp (seconds).
pub fn ago(then: u64) -> String {
    let now = rider_core::now_us() / 1_000_000;
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// `2E1B-049F-6F4C-840D` from a hex fingerprint.
pub fn short_fp(hex: &str) -> String {
    rider_core::identity::fp_from_hex(hex).map(|f| rider_core::identity::fp_short(&f)).unwrap_or_default()
}

/// Apply the app's look: a bit more air than egui's compact defaults.
pub fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 5.0);
        s.spacing.interact_size.y = 26.0;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_naturally() {
        let now = rider_core::now_us() / 1_000_000;
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 125), "2 min ago");
        assert_eq!(ago(now - 3 * 3600), "3 h ago");
        assert_eq!(ago(now - 90_000), "yesterday");
        assert_eq!(ago(now - 5 * 86_400), "5 days ago");
    }
}
