use crate::widgets;
use eframe::egui::{self, Align, Layout, RichText, Ui};
use rider_core::i18n::tr;
use rider_core::trf;
use std::time::{Duration, Instant};

const MIN_SECONDS: u64 = 5;
const MAX_SECONDS: u64 = 300;

#[derive(Debug, PartialEq)]
pub enum TunnelEvent {
    Request { id: u64, target: String, seconds: u64 },
    Expired(u64),
    Opened(String),
}

pub fn parse(v: &serde_json::Value) -> Option<TunnelEvent> {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
    let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or_default();
    Some(match v.get("event")?.as_str()? {
        "tunnel_request" => TunnelEvent::Request { id: n("id"), target: s("target"), seconds: n("seconds") },
        "tunnel_request_expired" => TunnelEvent::Expired(n("id")),
        "tunnel_open" => TunnelEvent::Opened(s("target")),
        _ => return None,
    })
}

pub struct TunnelAsk {
    pub id: u64,
    pub target: String,
    until: Instant,
    total: f32,
}

impl TunnelAsk {
    pub fn new(id: u64, target: String, seconds: u64) -> Self {
        let seconds = seconds.clamp(MIN_SECONDS, MAX_SECONDS);
        Self { id, target, until: Instant::now() + Duration::from_secs(seconds), total: seconds as f32 }
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.until
    }

    pub fn reply(&self, allow: bool) -> String {
        format!("{} {}", if allow { "tunnel-allow" } else { "tunnel-deny" }, self.id)
    }
}

pub fn opened_notice(target: &str) -> String {
    trf!("A TCP tunnel to {target} is open.", "{target} ünvanına TCP tuneli açıqdır.")
}

pub fn banner(ui: &mut Ui, ask: &TunnelAsk, device: &str) -> Option<bool> {
    let left = ask.until.saturating_duration_since(Instant::now());
    ui.ctx().request_repaint_after(Duration::from_millis(250));
    let dark = ui.visuals().dark_mode;
    let fill = if dark { egui::Color32::from_rgb(58, 46, 20) } else { egui::Color32::from_rgb(255, 244, 214) };
    let mut choice = None;
    egui::Panel::top("tunnel-ask")
        .resizable(false)
        .show_separator_line(true)
        .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(14, 9)))
        .show(ui, |ui| {
            let text = trf!(
                "\"{device}\" wants a TCP tunnel to {}",
                "\"{device}\" {} ünvanına TCP tuneli istəyir",
                ask.target
            );
            ui.add(egui::Label::new(widgets::title(&text, 13.5)).wrap()).on_hover_text(tr(
                "Allowing lets them reach this address from their computer for the rest of the session.",
                "İcazə versəniz, seans bitənədək öz kompüterlərindən bu ünvana qoşula biləcəklər.",
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(trf!("Denied by itself in {} s", "{} san. sonra rədd ediləcək", left.as_secs() + 1))
                        .weak(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(tr("Deny", "Rədd et")).clicked() {
                        choice = Some(false);
                    }
                    let allow = egui::Button::new(
                        RichText::new(tr("Allow", "İcazə ver")).family(widgets::semibold()).color(egui::Color32::WHITE),
                    )
                    .fill(widgets::ACCENT);
                    if ui.add(allow).clicked() {
                        choice = Some(true);
                    }
                });
            });
            ui.add_space(6.0);
            let part = (left.as_secs_f32() / ask.total).clamp(0.0, 1.0);
            let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, 1.5, ui.visuals().widgets.noninteractive.bg_stroke.color);
            let done = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * part, bar.height()));
            ui.painter().rect_filled(done, 1.5, widgets::AMBER);
        });
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(line: &str) -> Option<TunnelEvent> {
        parse(&serde_json::from_str(line).unwrap())
    }

    #[test]
    fn events_and_replies() {
        assert_eq!(
            ev(r#"{"event":"tunnel_request","id":3,"target":"192.168.1.20:443","seconds":60}"#),
            Some(TunnelEvent::Request { id: 3, target: "192.168.1.20:443".into(), seconds: 60 })
        );
        assert_eq!(ev(r#"{"event":"tunnel_request_expired","id":3}"#), Some(TunnelEvent::Expired(3)));
        assert_eq!(ev(r#"{"event":"tunnel_open","target":"nas:80"}"#), Some(TunnelEvent::Opened("nas:80".into())));
        assert_eq!(ev(r#"{"event":"chat","text":"x"}"#), None);
        let ask = TunnelAsk::new(7, "nas:80".into(), 60);
        assert_eq!(ask.reply(true), "tunnel-allow 7");
        assert_eq!(ask.reply(false), "tunnel-deny 7");
        assert!(!ask.expired());
    }
}
