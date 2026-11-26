use super::{
    Access, Align, ChatAction, ClientApp, GREEN, Layout, Msg, SupportStage, Ui, egui, sys, tr, tray, trf, vec2, widgets,
};

impl ClientApp {
    pub(super) fn session_page(&mut self, ui: &mut Ui) -> bool {
        let Some(s) = &mut self.support else { return false };
        let SupportStage::Active { device } = s.stage.clone() else { return false };
        let mut close = false;
        let mut access_changed = false;
        let dark = ui.visuals().dark_mode;
        let bar = if dark { egui::Color32::from_rgb(30, 31, 36) } else { egui::Color32::from_rgb(232, 234, 239) };
        egui::Panel::top("session")
            .resizable(false)
            .show_separator_line(true)
            .frame(egui::Frame::new().fill(bar).inner_margin(egui::Margin::symmetric(14, 7)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = widgets::danger(ui, tr("End session", "Seansı bitir")).clicked();
                        ui.add_space(4.0);
                        access_changed = widgets::access_menu(ui, &mut s.access);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            widgets::dot(ui, Some(GREEN));
                            let what = if s.access == Access::ViewOnly {
                                tr("They can see this screen.", "Bu ekranı görə bilər.")
                            } else {
                                tr("They can see and control this computer.", "Bu kompüteri görə və idarə edə bilər.")
                            };
                            let who = trf!("\"{device}\" is connected", "\"{device}\" qoşulub");
                            ui.add(egui::Label::new(widgets::title(&who, 14.0)).truncate()).on_hover_text(what);
                        });
                    });
                });
            });
        let mut chat_actions = Vec::new();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ui.visuals().panel_fill)).show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let w = full.width().min(620.0);
            let rect = egui::Rect::from_center_size(full.center(), vec2(w, full.height()));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| chat_actions = s.chat.show(ui));
        });
        if access_changed {
            let line = format!("access {}", s.access.code());
            s.send(&line);
            s.chat.files_allowed = s.access.permissions().files;
            s.chat.notice(trf!("Access changed: {}", "Giriş dəyişdi: {}", widgets::access_name(s.access)));
        }
        self.support_chat(chat_actions);
        if close {
            if let Some(s) = &mut self.support {
                s.send("quit");
            }
            self.support = None;
        }
        true
    }

    pub(super) fn support_chat(&mut self, actions: Vec<ChatAction>) {
        let Some(s) = &mut self.support else { return };
        for a in actions {
            match a {
                ChatAction::Send(text) => {
                    s.chat.text(true, &text);
                    s.send_json("chat", &text);
                }
                ChatAction::Attach => {
                    let (id, tx) = (s.id, self.bus.tx());
                    std::thread::spawn(move || {
                        let files = rider_ui::files::pick_files();
                        if !files.is_empty() {
                            tx.send(Msg::SupportFiles { id, files });
                        }
                    });
                }
                ChatAction::Cancel(id) => s.send(&format!("cancel-file {id}")),
                ChatAction::Open(id) => {
                    if let Some(p) = s.files.get(&id)
                        && let Err(e) = rider_ui::files::open(p)
                    {
                        let text = format!("{}: {e}", p.display());
                        s.chat.notice(text);
                    }
                }
                ChatAction::SaveAs(fid) => {
                    let (Some(path), Some(f)) = (s.files.get(&fid).cloned(), s.chat.file(fid)) else { continue };
                    let (name, id, tx) = (f.name.clone(), s.id, self.bus.tx());
                    std::thread::spawn(move || {
                        let text = match rider_ui::files::save_copy(&path, &name) {
                            Ok(Some(to)) => format!("{} {}", tr("Saved to", "Saxlanıldı:"), to.display()),
                            Ok(None) => return,
                            Err(e) => format!("{name}: {e}"),
                        };
                        tx.send(Msg::SupportNotice { id, text });
                    });
                }
                ChatAction::Close => {}
            }
        }
    }

    pub(super) fn close_to_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        tray::hide_main();
        let hint = "TrayHintShown";
        if !sys::flag(hint) {
            sys::set_flag(hint);
            tray.notify(
                tr("Remote Desktop Rider is still running", "Remote Desktop Rider işləməyə davam edir"),
                tr(
                    "It stays here in the notification area. Right-click the icon and choose Quit to exit.",
                    "Proqram bildiriş sahəsində qalır. Çıxmaq üçün nişanın üzərində sağ klikləyib «Çıxış» seçin.",
                ),
            );
        }
    }
}
