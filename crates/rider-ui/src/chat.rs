//! The chat of a session: messages and files between the two people, shown
//! the same way in the session window (a panel beside the remote screen) and
//! on the controlled computer (the app's session window).
//!
//! Only state and drawing live here. The owner feeds in what happens
//! (`text`, `file_*`, `notice`) and acts on what [`Chat::show`] returns
//! (send a message, pick a file, open or save one). Nothing is written to
//! disk: when the session ends the `Chat` is dropped with everything in it.

use crate::glyph::{self, Glyph};
use crate::theme::{ACCENT, GREY, border, card_fill, semibold};
use egui::{
    Align, Button, Color32, CornerRadius, Frame, Key, Label, Layout, Margin, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Ui, vec2,
};
use rider_core::i18n::tr;
use rider_core::trf;

/// Longest message accepted (characters), on both sides.
pub const MAX_TEXT: usize = 4000;

#[derive(Debug, Clone, PartialEq)]
pub enum FileState {
    /// Bytes moved so far.
    Moving(u64),
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: u64,
    /// Sent from this side.
    pub mine: bool,
    pub name: String,
    pub size: u64,
    pub state: FileState,
    at: String,
}

#[derive(Debug, Clone)]
enum Item {
    Text {
        mine: bool,
        text: String,
        at: String,
    },
    File(FileEntry),
    /// A line from the app itself: "Access changed to View only".
    Notice(String),
}

/// What the person did in the chat; the owner carries it out.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Send(String),
    /// Pick files to send (the owner opens its file dialog).
    Attach,
    Cancel(u64),
    Open(u64),
    SaveAs(u64),
    Close,
}

pub struct Chat {
    /// The other side's name, for the header.
    pub peer: String,
    items: Vec<Item>,
    draft: String,
    /// Whether files may be sent (the access level allows them).
    pub files_allowed: bool,
    /// Messages that arrived while the chat was not on screen.
    pub unread: usize,
    /// Show the close button (the session window's panel can close).
    pub closable: bool,
    /// Show "Chat" and the other side's name at the top (off where the
    /// page around it already says who it is).
    pub heading: bool,
    focus_input: bool,
}

impl Chat {
    pub fn new(peer: impl Into<String>) -> Self {
        Self {
            peer: peer.into(),
            items: Vec::new(),
            draft: String::new(),
            files_allowed: true,
            unread: 0,
            closable: true,
            heading: true,
            focus_input: true,
        }
    }

    /// A message from either side. Over-long text (from a modified peer) is cut.
    pub fn text(&mut self, mine: bool, text: &str) {
        let text: String = text.chars().take(MAX_TEXT).collect();
        if !mine {
            self.unread += 1;
        }
        self.items.push(Item::Text { mine, text, at: now_hm() });
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.items.push(Item::Notice(text.into()));
    }

    pub fn file_started(&mut self, id: u64, mine: bool, name: &str, size: u64) {
        if !mine {
            self.unread += 1;
        }
        self.items.push(Item::File(FileEntry {
            id,
            mine,
            name: name.to_owned(),
            size,
            state: FileState::Moving(0),
            at: now_hm(),
        }));
    }

    pub fn file_progress(&mut self, id: u64, done: u64) {
        if let Some(f) = self.file_mut(id)
            && matches!(f.state, FileState::Moving(_))
        {
            f.state = FileState::Moving(done.min(f.size));
        }
    }

    pub fn file_done(&mut self, id: u64) {
        if let Some(f) = self.file_mut(id) {
            f.state = FileState::Done;
        }
    }

    pub fn file_failed(&mut self, id: u64, reason: impl Into<String>) {
        if let Some(f) = self.file_mut(id) {
            f.state = FileState::Failed(reason.into());
        }
    }

    pub fn file(&self, id: u64) -> Option<&FileEntry> {
        self.items.iter().find_map(|i| match i {
            Item::File(f) if f.id == id => Some(f),
            _ => None,
        })
    }

    fn file_mut(&mut self, id: u64) -> Option<&mut FileEntry> {
        self.items.iter_mut().find_map(|i| match i {
            Item::File(f) if f.id == id => Some(f),
            _ => None,
        })
    }

    /// Put the text cursor in the message box on the next frame.
    pub fn focus(&mut self) {
        self.focus_input = true;
    }

    /// Whether the chat has the keyboard (the message box or a button has
    /// focus): keys then act here, not on the remote computer.
    pub fn typing(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.focused().is_some())
    }

    /// Draw the whole chat into `ui` (header, messages, message box); returns
    /// what the person did.
    pub fn show(&mut self, ui: &mut Ui) -> Vec<Action> {
        self.unread = 0;
        let mut actions = Vec::new();
        let dark = ui.visuals().dark_mode;
        Frame::new().fill(ui.visuals().panel_fill).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            // Header, lines and message box sit flush; each sets its own margins.
            ui.spacing_mut().item_spacing.y = 0.0;
            // Header: one line, only where the page around it does not
            // already say who it is.
            if self.heading || self.closable {
                Frame::new().inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 6 }).show(ui, |ui| {
                    let closable = self.closable;
                    let heading = self.heading;
                    let peer = &self.peer;
                    // From the right: the close button keeps its place, the
                    // name is cut to what is left. A fixed row height: a
                    // layout inside `horizontal` would take the full height.
                    let row = vec2(ui.available_width(), 24.0);
                    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                        if closable {
                            let close = glyph::button(ui, Glyph::Close, 24.0, true, None, GREY);
                            if close.on_hover_text(tr("Close the chat", "Söhbəti bağla")).clicked() {
                                actions.push(Action::Close);
                            }
                        }
                        privacy_mark(ui);
                        if heading {
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.label(RichText::new(tr("Chat", "Söhbət")).family(semibold()).size(14.0));
                                ui.add(Label::new(RichText::new(peer).size(12.0).weak()).truncate());
                            });
                        }
                    });
                });
                separator(ui, dark);
            }

            // The message box is laid out from the bottom up, the messages fill the rest.
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                Frame::new().inner_margin(Margin::symmetric(8, 7)).show(ui, |ui| self.composer(ui, &mut actions));
                separator(ui, dark);
                ui.with_layout(Layout::top_down(Align::Min), |ui| self.messages(ui, &mut actions));
            });
        });
        actions
    }

    fn messages(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.add_space(6.0);
            if self.items.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(ui.available_width().min(260.0));
                    glyph::show(ui, Glyph::Bubble, 28.0, GREY);
                    ui.add_space(4.0);
                    ui.add(
                        Label::new(
                            RichText::new(tr(
                                "Messages and files appear here. Encrypted, never saved, gone when the session ends.",
                                "Mesajlar və fayllar burada görünür. Şifrələnir, saxlanılmır, seans bitəndə silinir.",
                            ))
                            .size(11.5)
                            .weak(),
                        )
                        .wrap(),
                    );
                });
                return;
            }
            let width = ui.available_width();
            ui.spacing_mut().item_spacing.y = 0.0;
            for (n, item) in self.items.iter().enumerate() {
                // Messages from one side in the same minute form a group:
                // close together, the time only under the last one.
                let next = self.items.get(n + 1);
                let last_of_group = !matches!((item.side(), next.and_then(Item::side)),
                    (Some(a), Some(b)) if a == b);
                match item {
                    Item::Text { mine, text, at } => bubble(ui, width, *mine, last_of_group.then_some(at), |ui| {
                        let color = if *mine { Color32::WHITE } else { ui.visuals().text_color() };
                        ui.add(Label::new(RichText::new(text).size(13.5).color(color)).wrap().selectable(true));
                    }),
                    Item::File(f) => {
                        bubble(ui, width, f.mine, last_of_group.then_some(&f.at), |ui| file_card(ui, f, actions));
                    }
                    Item::Notice(text) => {
                        ui.add_space(4.0);
                        ui.vertical_centered(|ui| {
                            ui.add(Label::new(RichText::new(text).size(11.0).weak()).wrap());
                        });
                        ui.add_space(6.0);
                    }
                }
            }
            ui.add_space(4.0);
        });
    }

    fn composer(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        // Plain Enter sends: taken before the text box sees it, so the box
        // keeps its focus. Shift+Enter is the box's own new-line key.
        let focused = ui.memory(|m| m.has_focus(input_id()));
        let enter = focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        let ready = !self.draft.trim().is_empty();
        const BUTTON: f32 = 28.0;
        ui.spacing_mut().button_padding = vec2(4.0, 4.0);
        // A fixed row height (a layout inside `horizontal` would take the
        // full height); the buttons stay at the bottom as the message grows.
        let row = vec2(ui.available_width(), BUTTON);
        ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let fg = ui.visuals().text_color();
            let attach = glyph::button(ui, Glyph::Plus, BUTTON, self.files_allowed, None, fg);
            let tip = if self.files_allowed {
                tr("Send a file", "Fayl göndər")
            } else {
                tr("Files are not allowed in this session", "Bu seansda fayllara icazə verilməyib")
            };
            if attach.on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                actions.push(Action::Attach);
            }
            // Then the message box, exactly as wide as what is left beside
            // Send (the width includes its inner margin).
            let margin = Margin::symmetric(8, 5);
            let gap = ui.spacing().item_spacing.x;
            let width = ui.available_width() - BUTTON - gap;
            let edit = TextEdit::multiline(&mut self.draft)
                .id(input_id())
                .hint_text(tr("Message", "Mesaj"))
                .font(egui::FontId::proportional(13.5))
                .desired_rows(1)
                .desired_width(width.max(40.0))
                .char_limit(MAX_TEXT)
                .return_key(Some(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, Key::Enter)))
                .margin(margin);
            let resp = ui.add(edit);
            let fill = if ready { ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill };
            let send = glyph::button(ui, Glyph::Send, BUTTON, ready, Some(fill), Color32::WHITE);
            let clicked = send.on_hover_text(tr("Send (Enter)", "Göndər (Enter)")).clicked();
            if std::mem::take(&mut self.focus_input) {
                resp.request_focus();
            }
            if (enter || clicked) && ready {
                actions.push(Action::Send(self.draft.trim().to_owned()));
                self.draft.clear();
                resp.request_focus();
            }
        });
    }
}

impl Item {
    /// Who it is from and when (for grouping); notices stand alone.
    fn side(&self) -> Option<(bool, &str)> {
        match self {
            Item::Text { mine, at, .. } => Some((*mine, at)),
            Item::File(f) => Some((f.mine, &f.at)),
            Item::Notice(_) => None,
        }
    }
}

/// A small lock; hovering it says how the chat is kept private.
fn privacy_mark(ui: &mut Ui) {
    glyph::show(ui, Glyph::Lock, 16.0, GREY).on_hover_text(tr(
        "End-to-end encrypted. Nothing is kept after the session.",
        "Tam şifrələnib. Seansdan sonra heç nə saxlanılmır.",
    ));
}

fn input_id() -> egui::Id {
    egui::Id::unique("rider-chat-input")
}

fn separator(ui: &mut Ui, dark: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, border(dark));
}

/// A speech bubble: right and blue for this side, left and grey for the
/// other. `at` is shown under the last bubble of a group.
fn bubble(ui: &mut Ui, width: f32, mine: bool, at: Option<&String>, add: impl FnOnce(&mut Ui)) {
    let max = (width * 0.82).max(120.0);
    let dark = ui.visuals().dark_mode;
    let (fill, stroke) = if mine { (ACCENT, Stroke::NONE) } else { (card_fill(ui), Stroke::new(1.0, border(dark))) };
    let r = 10;
    // The last bubble of a group gets a sharper corner on the sender's side, like a tail.
    let tail = if at.is_some() { 3 } else { r };
    let corners = if mine {
        CornerRadius { nw: r, ne: r, sw: r, se: tail }
    } else {
        CornerRadius { nw: r, ne: r, sw: tail, se: r }
    };
    // One column, aligned to the sender's side: the bubble and its time.
    Frame::new().inner_margin(Margin::symmetric(8, 0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.with_layout(Layout::top_down(if mine { Align::Max } else { Align::Min }), |ui| {
            Frame::new().fill(fill).stroke(stroke).corner_radius(corners).inner_margin(Margin::symmetric(9, 5)).show(
                ui,
                |ui| {
                    ui.set_max_width(max - 18.0);
                    add(ui);
                },
            );
            match at {
                Some(at) => {
                    ui.add_space(1.0);
                    ui.label(RichText::new(at).size(10.0).weak());
                    ui.add_space(6.0);
                }
                None => ui.add_space(2.0),
            }
        });
    });
}

fn file_card(ui: &mut Ui, f: &FileEntry, actions: &mut Vec<Action>) {
    let fg = if f.mine { Color32::WHITE } else { ui.visuals().text_color() };
    let weak = if f.mine { Color32::from_white_alpha(200) } else { ui.visuals().weak_text_color() };
    // A fixed width, the same on both sides; left to right inside, also in
    // the right-aligned column of this side (egui would flip rows there).
    ui.set_width(ui.available_width().min(230.0));
    ui.with_layout(Layout::top_down(Align::Min), |ui| file_card_body(ui, f, fg, weak, actions));
}

fn file_card_body(ui: &mut Ui, f: &FileEntry, fg: Color32, weak: Color32, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
    ui.horizontal(|ui| {
        glyph::show(ui, Glyph::File, 22.0, fg);
        ui.vertical(|ui| {
            ui.add(Label::new(RichText::new(&f.name).size(13.0).family(semibold()).color(fg)).truncate());
            let status = match &f.state {
                FileState::Moving(done) => {
                    trf!("{} of {}", "{} / {}", human_size(*done), human_size(f.size))
                }
                FileState::Done if f.mine => format!("{} · {}", human_size(f.size), tr("Sent", "Göndərildi")),
                FileState::Done => human_size(f.size),
                FileState::Failed(why) => why.clone(),
            };
            let color = if matches!(f.state, FileState::Failed(_)) { ui.visuals().error_fg_color } else { weak };
            ui.add(Label::new(RichText::new(status).size(11.0).color(color)).wrap());
        });
    });
    if let FileState::Moving(done) = f.state {
        let frac = if f.size == 0 { 1.0 } else { done as f32 / f.size as f32 };
        ui.horizontal(|ui| {
            ui.add(egui::ProgressBar::new(frac).desired_height(3.0).desired_width(ui.available_width().min(160.0)));
            if ui.add(Button::new(RichText::new(tr("Cancel", "Ləğv et")).size(11.0).color(fg)).frame(false)).clicked()
            {
                actions.push(Action::Cancel(f.id));
            }
        });
    }
    if f.state == FileState::Done && !f.mine {
        ui.horizontal(|ui| {
            if ui.small_button(tr("Open", "Aç")).clicked() {
                actions.push(Action::Open(f.id));
            }
            if ui.small_button(tr("Save as…", "Yadda saxla…")).clicked() {
                actions.push(Action::SaveAs(f.id));
            }
        });
    }
}

/// "1.4 MB", "820 KB", "12 bytes".
pub fn human_size(n: u64) -> String {
    const KB: f64 = 1024.0;
    let f = n as f64;
    if f >= KB * KB * KB {
        format!("{:.1} GB", f / (KB * KB * KB))
    } else if f >= KB * KB {
        format!("{:.1} MB", f / (KB * KB))
    } else if f >= KB {
        format!("{:.0} KB", f / KB)
    } else {
        trf!("{} bytes", "{} bayt", n)
    }
}

/// The local time as "14:05".
fn now_hm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(1536 * 1024), "1.5 MB");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn messages_and_files() {
        let mut c = Chat::new("Laptop");
        c.text(false, "hello");
        c.text(true, "hi");
        assert_eq!(c.unread, 1, "only the other side's messages count");
        c.file_started(7, false, "log.txt", 100);
        c.file_progress(7, 40);
        assert_eq!(c.file(7).unwrap().state, FileState::Moving(40));
        c.file_progress(7, 400);
        assert_eq!(c.file(7).unwrap().state, FileState::Moving(100), "never past the size");
        c.file_done(7);
        c.file_progress(7, 10);
        assert_eq!(c.file(7).unwrap().state, FileState::Done, "late progress does not undo done");
        c.file_failed(8, "nope");
        assert!(c.file(8).is_none());
        let long = "x".repeat(MAX_TEXT + 50);
        c.text(false, &long);
        match c.items.last() {
            Some(Item::Text { text, .. }) => assert_eq!(text.chars().count(), MAX_TEXT),
            _ => panic!(),
        }
    }
}
