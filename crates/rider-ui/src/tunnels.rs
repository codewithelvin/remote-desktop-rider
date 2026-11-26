use crate::glyph::{self, Glyph};
use crate::theme::{ACCENT, GREY, border, card_fill, semibold};
use egui::{
    Align, Color32, CornerRadius, Frame, Key, Label, Layout, Margin, RichText, ScrollArea, Stroke, TextEdit, Ui, vec2,
};
use rider_core::i18n::tr;

const BUTTON: f32 = 28.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Add(String),
    Close(usize),
    Hide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub local: String,
    pub target: String,
}

#[derive(Default)]
pub struct Tunnels {
    draft: String,
    pub error: Option<String>,
    pub open: Vec<Open>,
}

fn input_id() -> egui::Id {
    egui::Id::unique("rider-tunnel-target")
}

impl Tunnels {
    pub fn show(&mut self, ui: &mut Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        let dark = ui.visuals().dark_mode;
        Frame::new().fill(ui.visuals().panel_fill).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing.y = 0.0;
            Frame::new().inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 6 }).show(ui, |ui| {
                ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), Layout::right_to_left(Align::Center), |ui| {
                    let close = glyph::button(ui, Glyph::Close, 24.0, true, None, GREY);
                    if close.on_hover_text(tr("Close", "Bağla")).clicked() {
                        actions.push(Action::Hide);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.label(RichText::new(tr("TCP tunnels", "TCP tunelləri")).family(semibold()).size(14.0));
                    });
                });
            });
            ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, border(dark)));
            Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add(
                    Label::new(
                        RichText::new(tr(
                            "Reach a port on that computer or its local network. Each new address is approved there.",
                            "O kompüterdə və ya onun yerli şəbəkəsində bir porta qoşulun. Hər yeni ünvan orada təsdiqlənir.",
                        ))
                        .size(11.5)
                        .weak(),
                    )
                    .wrap(),
                );
                self.composer(ui, &mut actions);
                if let Some(e) = &self.error {
                    ui.add(Label::new(RichText::new(e).size(11.5).color(Color32::from_rgb(0xd1, 0x3b, 0x3b))).wrap());
                }
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (i, t) in self.open.iter().enumerate() {
                        row(ui, i, t, dark, &mut actions);
                    }
                });
            });
        });
        actions
    }

    fn composer(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let focused = ui.memory(|m| m.has_focus(input_id()));
        let enter = focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        let ready = !self.draft.trim().is_empty();
        ui.allocate_ui_with_layout(vec2(ui.available_width(), BUTTON), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - BUTTON - ui.spacing().item_spacing.x;
            let resp = ui.add(
                TextEdit::singleline(&mut self.draft)
                    .id(input_id())
                    .hint_text("192.168.1.20:443")
                    .font(egui::FontId::proportional(13.5))
                    .desired_width(width.max(40.0))
                    .char_limit(rider_core::forward::MAX_TARGET_LEN)
                    .margin(Margin::symmetric(8, 5)),
            );
            let fill = if ready { ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill };
            let add = glyph::button(ui, Glyph::Plus, BUTTON, ready, Some(fill), Color32::WHITE);
            if (enter || add.on_hover_text(tr("Open the tunnel (Enter)", "Tuneli aç (Enter)")).clicked()) && ready {
                actions.push(Action::Add(self.draft.trim().to_owned()));
                self.draft.clear();
                resp.request_focus();
            }
        });
    }
}

fn row(ui: &mut Ui, i: usize, t: &Open, dark: bool, actions: &mut Vec<Action>) {
    Frame::new()
        .fill(card_fill(ui))
        .stroke(Stroke::new(1.0, border(dark)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), Layout::right_to_left(Align::Center), |ui| {
                let close = glyph::button(ui, Glyph::Close, 24.0, true, None, GREY);
                if close.on_hover_text(tr("Close the tunnel", "Tuneli bağla")).clicked() {
                    actions.push(Action::Close(i));
                }
                if ui.small_button(tr("Copy", "Kopyala")).clicked() {
                    ui.ctx().copy_text(t.local.clone());
                }
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(Label::new(RichText::new(&t.local).size(13.0).family(semibold())).truncate());
                    ui.add(Label::new(RichText::new(format!("\u{2192} {}", t.target)).size(11.5).weak()).truncate());
                });
            });
        });
    ui.add_space(6.0);
}
