use super::{
    ACCENT, AMBER, Align, ClientApp, ClientConfig, Duration, GREEN, GREY, Instant, KnownHost, Layout, RELEASES,
    RemoteAccess, RichText, ScrollArea, Tab, TextEdit, Ui, config_mtime, egui, host_side, icon, progress_line, sys, tr,
    trf, updates, vec2, widgets,
};
use rider_core::rendezvous::ServerRef;

impl ClientApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new((logo.id(), vec2(28.0, 28.0))));
            }
            let room = ui.available_width() - 2.0;
            ui.label(widgets::title_fit(ui, "Remote Desktop Rider", 15.0, room));
        });
        ui.add_space(18.0);
        for (tab, ic, text) in [
            (Tab::Computers, icon::PC, tr("Computers", "Kompüterlər")),
            (Tab::ThisComputer, icon::HOME, tr("This computer", "Bu kompüter")),
            (Tab::Enterprise, icon::COMPANY, tr("Enterprise", "Müəssisə")),
            (Tab::Settings, icon::SETTINGS, tr("Settings", "Parametrlər")),
        ] {
            if tab == Tab::ThisComputer && !host_side() {
                continue;
            }
            if tab == Tab::Enterprise && self.cfg.enterprise.is_none() && self.tab != Tab::Enterprise {
                continue;
            }
            if widgets::nav_item(ui, ic, text, self.tab == tab) && self.tab != tab {
                self.tab = tab;
                self.page_since = ui.input(|i| i.time);
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(12.0);
            widgets::hint(ui, &format!("   {} {}", tr("Version", "Versiya"), env!("CARGO_PKG_VERSION")));
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (color, text) = match (&self.cfg.server, &self.server_error) {
                    (None, _) => (None, tr("Local network only", "Yalnız yerli şəbəkə")),
                    (Some(_), Some(_)) => (Some(AMBER), tr("Server offline", "Server əlçatmazdır")),
                    (Some(_), None) if self.polled.is_none() && !self.cfg.hosts.is_empty() => {
                        (Some(GREY), tr("Server…", "Server…"))
                    }
                    (Some(_), None) => (Some(GREEN), tr("Server online", "Server aktivdir")),
                };
                widgets::dot(ui, color);
                ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
            });
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                widgets::language_switch(ui);
            });
        });
    }

    pub(super) fn computers(&mut self, ui: &mut Ui) {
        let has_hosts = !self.cfg.hosts.is_empty();
        let mut add = false;
        let mut quick = false;
        widgets::page_header(
            ui,
            tr("Computers", "Kompüterlər"),
            tr("Computers this device can control.", "Bu cihazın idarə edə bildiyi kompüterlər."),
            |ui| {
                if has_hosts {
                    add = widgets::primary(
                        ui,
                        true,
                        &format!("{}   {}", icon::ADD, tr("Add computer", "Kompüter əlavə et")),
                    )
                    .clicked();
                }
                quick = ui
                    .button(format!("{}   {}", icon::PLAY, tr("Quick connect", "Sürətli qoşulma")))
                    .on_hover_text(tr(
                        "One-time session with the code another computer shows: no pairing, no PIN.",
                        "Digər kompüterin göstərdiyi kodla birdəfəlik seans: cütləşdirmə və PIN kod tələb olunmur.",
                    ))
                    .clicked();
            },
        );
        if let Some(e) = &self.server_error {
            widgets::card(ui, |ui| {
                widgets::error(
                    ui,
                    &trf!(
                        "The server can't be reached, so online status is unknown. {e}",
                        "Serverə qoşulmaq mümkün deyil, ona görə onlayn vəziyyət məlum deyil. {e}"
                    ),
                );
            });
            ui.add_space(8.0);
        }

        if !has_hosts {
            widgets::card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new(icon::PC).size(44.0).color(ACCENT));
                    ui.label(widgets::title(tr("No computers yet", "Hələ kompüter yoxdur"), 18.0));
                    widgets::hint(
                        ui,
                        tr(
                            "On the computer you want to control, open Remote Desktop Rider, go to\n\
                             This computer > Manage this computer, and click Add a device.\n\
                             For one-time help, use Quick connect instead.",
                            "İdarə etmək istədiyiniz kompüterdə Remote Desktop Rider-i açın,\n\
                             «Bu kompüter» > «Bu kompüteri idarə et» bölməsində «Cihaz əlavə et» düyməsini klikləyin.\n\
                             Birdəfəlik kömək üçün isə «Sürətli qoşulma»dan istifadə edin.",
                        ),
                    );
                    ui.add_space(4.0);
                    add = widgets::primary(
                        ui,
                        true,
                        &format!("{}   {}", icon::ADD, tr("Add computer", "Kompüter əlavə et")),
                    )
                    .clicked()
                        || add;
                    ui.add_space(18.0);
                });
            });
        }
        if add {
            self.open_add();
        }
        if quick {
            self.open_quick();
        }

        let mut hosts = self.cfg.hosts.clone();
        hosts.sort_by_key(|h| h.name.to_lowercase());
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            for h in &hosts {
                self.host_card(ui, h);
                ui.add_space(8.0);
            }
        });
    }

    pub(super) fn host_card(&mut self, ui: &mut Ui, h: &KnownHost) {
        let online = self.online.get(&h.fingerprint).copied();
        let session = self.sessions.iter().find(|s| s.host == h.fingerprint).map(|s| SessionLine {
            id: s.id,
            finished: s.finished,
            connected: s.connected.clone(),
            error: sys::error_text(&s.stderr),
        });
        let in_session = session.as_ref().is_some_and(|s| s.finished.is_none());
        let mut dismiss = None;
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::avatar(ui, icon::PC, online.map(|o| if o { GREEN } else { GREY }));
                ui.add_space(4.0);
                host_summary(ui, h, online);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.host_actions(ui, h, online, in_session));
            });
            if let Some(line) = &session {
                ui.add_space(2.0);
                ui.separator();
                dismiss = line.show(ui);
            }
        });
        if let Some(id) = dismiss {
            self.sessions.retain(|s| s.id != id);
        }
    }

    fn host_actions(&mut self, ui: &mut Ui, h: &KnownHost, online: Option<bool>, in_session: bool) {
        if self.remove.as_deref() == Some(h.fingerprint.as_str()) {
            if widgets::danger(ui, tr("Remove", "Sil")).clicked() {
                if let Err(e) = self.edit(|cfg| {
                    cfg.forget(&h.fingerprint);
                }) {
                    self.server_error = Some(e);
                }
                self.forget_pin(&h.fingerprint);
                self.remove = None;
            }
            if ui.button(tr("Cancel", "Ləğv et")).clicked() {
                self.remove = None;
            }
            ui.label(tr("Remove from this list?", "Siyahıdan silinsin?"));
        } else {
            if widgets::icon_button(ui, icon::DELETE, tr("Remove from this list", "Siyahıdan sil")).clicked() {
                self.remove = Some(h.fingerprint.clone());
            }
            let label = if in_session { tr("In session", "Seansdadır") } else { tr("Connect", "Qoşul") };
            let offline = online == Some(false);
            let connect = widgets::primary(ui, !in_session && !offline, label);
            if offline {
                connect.on_disabled_hover_text(tr(
                    "This computer is offline. Connect becomes available as soon as it is back online.",
                    "Bu kompüter oflayndır. Onlayn olan kimi «Qoşul» düyməsi aktivləşəcək.",
                ));
            } else if connect.clicked() {
                self.connect_to(h.fingerprint.clone(), h.name.clone());
            }
            if self.saved_pins.contains(&h.fingerprint)
                && widgets::icon_button(
                    ui,
                    icon::KEY,
                    tr(
                        "PIN remembered on this device. Click to forget it.",
                        "PIN kod bu cihazda yadda saxlanılıb. Silmək üçün klikləyin.",
                    ),
                )
                .clicked()
            {
                self.forget_pin(&h.fingerprint);
            }
        }
    }

    pub(super) fn this_computer(&mut self, ui: &mut Ui) {
        if self.remote_checked.elapsed() > Duration::from_secs(3) {
            self.remote = sys::remote_access();
            self.remote_checked = Instant::now();
        }
        widgets::page_header(ui, tr("This computer", "Bu kompüter"), &format!("\"{}\"", self.cfg.device_name), |_| {});
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            self.help_card(ui);
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::SHIELD, tr("Remote access", "Uzaqdan giriş"));
                ui.horizontal(|ui| {
                    let (color, state, text) = match self.remote {
                        RemoteAccess::On => (
                            GREEN,
                            tr("On", "Aktiv"),
                            tr(
                                "Devices you added can control this computer, even at the sign-in screen.",
                                "Əlavə etdiyiniz cihazlar bu kompüteri, hətta giriş ekranında da idarə edə bilər.",
                            ),
                        ),
                        RemoteAccess::Stopped => (
                            AMBER,
                            tr("Stopped", "Dayandırılıb"),
                            tr("Remote access is set up but not running.", "Uzaqdan giriş quraşdırılıb, lakin işləmir."),
                        ),
                        RemoteAccess::Off => (
                            GREY,
                            tr("Off", "Söndürülüb"),
                            tr("Nobody can connect to this computer.", "Heç kim bu kompüterə qoşula bilməz."),
                        ),
                    };
                    widgets::pill(ui, state, color);
                    ui.add(egui::Label::new(text).wrap());
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if widgets::primary(
                        ui,
                        true,
                        &format!("{}   {}", icon::SHIELD, tr("Manage this computer…", "Bu kompüteri idarə et…")),
                    )
                    .clicked()
                    {
                        self.manage_error = sys::relaunch_elevated("--manage").err().map(|e| format!("{e:#}"));
                    }
                    widgets::hint(ui, tr("Windows asks for permission once.", "Windows bir dəfə icazə istəyəcək."));
                });
                if let Some(e) = &self.manage_error {
                    widgets::error(ui, e);
                }
            });
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(
                    ui,
                    icon::INFO,
                    tr(
                        "Let another device control this computer",
                        "Başqa cihazın bu kompüteri idarə etməsinə icazə verin",
                    ),
                );
                for (n, step) in [
                    tr("Click Manage this computer above.", "Yuxarıda «Bu kompüteri idarə et» düyməsini klikləyin."),
                    tr("Set a PIN and turn on remote access.", "PIN kod təyin edin və uzaqdan girişi aktivləşdirin."),
                    tr(
                        "Click Add a device, then on the other device click Add computer and enter the code shown.",
                        "«Cihaz əlavə et» düyməsini klikləyin, sonra digər cihazda «Kompüter əlavə et» seçib göstərilən kodu daxil edin.",
                    ),
                ]
                .iter()
                .enumerate()
                {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(format!("{}.", n + 1)).color(ACCENT).strong());
                        ui.add(egui::Label::new(*step).wrap());
                    });
                }
            });
        });
    }

    pub(super) fn help_card(&mut self, ui: &mut Ui) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::KEY, tr("Get help now (one-time)", "İndi kömək alın (birdəfəlik)"));
            ui.add(
                egui::Label::new(tr(
                    "Let someone see and control this screen once, without a PIN or adding their device. \
                     You approve the connection, and nothing is kept afterwards.",
                    "Kiminsə bu ekranı bir dəfə görməsinə və idarə etməsinə icazə verin: PIN kod və cihaz əlavə \
                     etmək lazım deyil. Qoşulmanı siz təsdiqləyirsiniz, seansdan sonra heç nə saxlanılmır.",
                ))
                .wrap(),
            );
            let has_server = self.cfg.server.is_some();
            ui.horizontal(|ui| {
                let busy = self.support.is_some();
                if widgets::primary(ui, has_server && !busy, tr("Get a one-time code", "Birdəfəlik kod al")).clicked()
                {
                    self.start_support();
                }
                if !has_server {
                    widgets::hint(
                        ui,
                        tr(
                            "Needs a server: set it in Manage this computer → Server.",
                            "Server tələb olunur: onu «Bu kompüteri idarə et» → «Server» bölməsində təyin edin.",
                        ),
                    );
                }
            });
        });
    }

    pub(super) fn settings(&mut self, ui: &mut Ui) {
        widgets::page_header(ui, tr("Settings", "Parametrlər"), "", |_| {});
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::PC, tr("This device", "Bu cihaz"));
                egui::Grid::new("device").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                    widgets::hint(ui, tr("Name", "Ad"));
                    ui.label(&self.cfg.device_name);
                    ui.end_row();
                    widgets::hint(ui, tr("Fingerprint", "Barmaq izi"));
                    let fp = self
                        .identity
                        .as_ref()
                        .map_or_default(|i| rider_core::identity::fp_short(&i.fingerprint()));
                    ui.horizontal(|ui| widgets::copyable(ui, &fp, RichText::new(&fp).monospace()));
                    ui.end_row();
                });
            });
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::GLOBE, tr("Language", "Dil"));
                ui.horizontal(|ui| {
                    widgets::language_switch(ui);
                    ui.label(rider_core::i18n::lang().native_name());
                });
                widgets::hint(
                    ui,
                    if host_side() {
                        tr(
                            "English or Azerbaijani. Applies right away, also to session windows and Manage this computer.",
                            "İngilis və ya Azərbaycan dili. Dərhal tətbiq olunur: seans pəncərələri və «Bu kompüteri idarə et» də bu dildə açılır.",
                        )
                    } else {
                        tr(
                            "English or Azerbaijani. Applies right away, also to session windows.",
                            "İngilis və ya Azərbaycan dili. Dərhal tətbiq olunur: seans pəncərələri də bu dildə açılır.",
                        )
                    },
                );
            });
            ui.add_space(10.0);
            self.server_card(ui);
            ui.add_space(10.0);
            self.updates_card(ui);
            if self.cfg.enterprise.is_none() {
                ui.add_space(10.0);
                widgets::card(ui, |ui| {
                    widgets::section(ui, icon::COMPANY, tr("Enterprise mode", "Müəssisə rejimi"));
                    widgets::hint(
                        ui,
                        tr(
                            "For the company's admin: look after all company computers from this app.",
                            "Şirkətin admini üçün: bütün şirkət kompüterlərinə bu proqramdan baxın.",
                        ),
                    );
                    if ui.button(tr("Sign in…", "Daxil ol…")).clicked() {
                        self.tab = Tab::Enterprise;
                        self.page_since = ui.input(|i| i.time);
                    }
                });
            }
            if !host_side() {
                return;
            }
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::PLAY, tr("Startup", "Avtomatik başlatma"));
                if widgets::checkbox(
                    ui,
                    true,
                    &mut self.autostart,
                    tr("Start Remote Desktop Rider when I sign in", "Sistemə daxil olduqda Remote Desktop Rider-i başlat"),
                )
                .changed()
                    && let Err(e) = sys::set_autostart(self.autostart)
                {
                    self.autostart = sys::autostart();
                    self.settings_msg = Some(Err(format!("{e:#}")));
                }
                widgets::hint(
                    ui,
                    tr(
                        "Closing the window keeps the app in the notification area. Use Quit in its menu to exit.",
                        "Pəncərə bağlandıqda proqram bildiriş sahəsində qalır. Tam çıxmaq üçün onun menyusunda «Çıxış» seçin.",
                    ),
                );
                if let Some(Err(e)) = &self.settings_msg {
                    widgets::error(ui, e);
                }
            });
        });
    }

    pub(super) fn enterprise_page(&mut self, ui: &mut Ui) {
        let handle = self.rt.handle().clone();
        let env = crate::enterprise::Env {
            rt: &handle,
            identity: self.identity.as_ref(),
            server: self.cfg.server.as_ref(),
            admin: self.cfg.enterprise.as_ref(),
        };
        let outcome = self.enterprise.show(ui, &env);
        let saved = match outcome {
            Some(crate::enterprise::Outcome::SignedIn(admin)) => self.edit(|c| c.enterprise = Some(admin)),
            Some(crate::enterprise::Outcome::SignedOut) => self.edit(|c| c.enterprise = None),
            Some(crate::enterprise::Outcome::OpenSettings) => {
                self.tab = Tab::Settings;
                Ok(())
            }
            None => Ok(()),
        };
        if let Err(e) = saved {
            self.settings_msg = Some(Err(e));
        }
    }

    pub(super) fn update_banner(&mut self, ui: &mut Ui) {
        use updates::State;
        let state = self.updater.state();
        let release = match &state {
            State::Available(r)
            | State::Downloading { release: r, .. }
            | State::Installing(r)
            | State::Failed { release: Some(r), .. } => r.clone(),
            _ => return,
        };
        let version = release.version.to_string();
        let failed = matches!(state, State::Failed { .. });
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let color = if failed { ui.visuals().error_fg_color } else { ACCENT };
                ui.label(RichText::new(icon::DOWNLOAD).size(18.0).color(color));
                let title = match &state {
                    State::Downloading { .. } => trf!("Downloading version {version}", "{version} versiyası yüklənir"),
                    State::Installing(_) => trf!("Installing version {version}", "{version} versiyası quraşdırılır"),
                    State::Failed { .. } => {
                        trf!("Version {version} could not be installed", "{version} versiyası quraşdırıla bilmədi")
                    }
                    _ => trf!("Version {version} is available", "{version} versiyası mövcuddur"),
                };
                ui.label(RichText::new(title).strong());
                ui.hyperlink_to(tr("What's new", "Yeniliklər"), &release.page);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &state {
                    State::Downloading { done, .. } => {
                        let pct = *done as f32 * 100.0 / release.size.max(1) as f32;
                        ui.label(RichText::new(format!("{pct:.0}%")).strong().color(ACCENT));
                        let of = trf!(
                            "{} of {}",
                            "{} / {}",
                            rider_ui::chat::human_size(*done),
                            rider_ui::chat::human_size(release.size)
                        );
                        ui.label(RichText::new(of).weak());
                    }
                    State::Installing(_) => {
                        ui.add(egui::Spinner::new().size(16.0));
                    }
                    _ => {
                        let label = if failed {
                            tr("Try again", "Yenidən cəhd et")
                        } else {
                            tr("Install update", "Yeniləməni quraşdır")
                        };
                        if widgets::primary(ui, true, label).clicked() {
                            self.updater.install(release.clone());
                        }
                    }
                });
            });
            match &state {
                State::Downloading { done, .. } => {
                    progress_line(ui, Some(*done as f32 / release.size.max(1) as f32));
                    widgets::hint(
                        ui,
                        tr(
                            "Windows asks for permission once the download is done.",
                            "Yükləmə bitəndə Windows bir dəfə icazə istəyəcək.",
                        ),
                    );
                }
                State::Installing(_) => {
                    progress_line(ui, None);
                    widgets::hint(
                        ui,
                        tr(
                            "Allow it if Windows asks. The app reopens by itself; sessions from this device end.",
                            "Windows soruşsa, icazə verin. Proqram özü yenidən açılacaq; bu cihazdan seanslar bağlanacaq.",
                        ),
                    );
                }
                State::Failed { error, .. } => widgets::error(ui, error),
                _ => widgets::hint(
                    ui,
                    tr(
                        "Windows asks for permission once. Open sessions from this device end while it installs.",
                        "Windows bir dəfə icazə istəyəcək. Quraşdırma zamanı bu cihazdan açılmış seanslar bağlanacaq.",
                    ),
                ),
            }
        });
        ui.add_space(12.0);
    }

    pub(super) fn updates_card(&mut self, ui: &mut Ui) {
        use updates::State;
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::DOWNLOAD, tr("Updates", "Yeniləmələr"));
            ui.horizontal(|ui| {
                widgets::hint(ui, tr("Version", "Versiya"));
                ui.label(updates::current().to_string());
            });
            if !host_side() {
                ui.hyperlink_to(
                    tr(
                        "Get new versions from the releases page",
                        "Yeni versiyaları buraxılışlar səhifəsindən yükləyin",
                    ),
                    RELEASES,
                );
                return;
            }
            if !self.updater.installed {
                widgets::hint(
                    ui,
                    tr(
                        "Updates apply to the installed app (Program Files); this copy runs from elsewhere.",
                        "Yeniləmələr quraşdırılmış proqrama (Program Files) tətbiq olunur; bu nüsxə başqa qovluqdan işləyir.",
                    ),
                );
                return;
            }
            let mut auto = self.updater.auto;
            if widgets::checkbox(
                ui,
                true,
                &mut auto,
                tr("Check for updates automatically", "Yeniləmələri avtomatik yoxla"),
            )
            .changed()
            {
                self.updater.set_auto(auto);
            }
            ui.horizontal(|ui| {
                let state = self.updater.state();
                let checking = matches!(state, State::Checking { .. });
                let manual = matches!(state, State::Checking { manual: true });
                if widgets::secondary_busy(ui, !checking, manual, tr("Check now", "İndi yoxla")).clicked() {
                    self.updater.check();
                }
                match state {
                    State::UpToDate => {
                        widgets::success(ui, tr("You have the latest version.", "Ən son versiya quraşdırılıb."));
                    }
                    State::Failed { error, release: None } => widgets::error(ui, &error),
                    State::Available(r) => {
                        let version = &r.version;
                        widgets::hint(
                            ui,
                            &trf!(
                                "Version {version} is available (see the top).",
                                "{version} versiyası mövcuddur (yuxarıya baxın)."
                            ),
                        );
                    }
                    _ => {}
                }
            });
            widgets::hint(
                ui,
                tr(
                    "Updates come from this project's GitHub releases and are checked against their published SHA-256.",
                    "Yeniləmələr layihənin GitHub buraxılışlarından yüklənir və dərc olunmuş SHA-256 ilə yoxlanılır.",
                ),
            );
        });
    }

    pub(super) fn server_card(&mut self, ui: &mut Ui) {
        if !host_side() {
            return self.server_editor(ui);
        }
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::GLOBE, tr("Server", "Server"));
            match &self.cfg.server {
                Some(s) => {
                    egui::Grid::new("server").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                        widgets::hint(ui, tr("Address", "Ünvan"));
                        ui.label(&s.addr);
                        ui.end_row();
                        widgets::hint(ui, tr("Fingerprint", "Barmaq izi"));
                        ui.label(RichText::new(widgets::short_fp(&s.fingerprint)).monospace());
                        ui.end_row();
                    });
                }
                None => widgets::hint(
                    ui,
                    tr(
                        "Not set: only computers on this network can be reached.",
                        "Təyin edilməyib: yalnız bu şəbəkədəki kompüterlərə qoşulmaq mümkündür.",
                    ),
                ),
            }
            ui.horizontal(|ui| {
                if ui
                    .button(format!(
                        "{}   {}",
                        icon::SHIELD,
                        tr("Change in Manage this computer…", "«Bu kompüteri idarə et» bölməsində dəyiş…")
                    ))
                    .clicked()
                {
                    self.manage_error = sys::relaunch_elevated("--manage").err().map(|e| format!("{e:#}"));
                }
                widgets::hint(ui, tr("One setting for both directions.", "Hər iki istiqamət üçün bir ayar."));
            });
            if let Some(e) = &self.manage_error {
                widgets::error(ui, e);
            }
        });
    }

    pub(super) fn server_editor(&mut self, ui: &mut Ui) {
        let saved = self.cfg.server.as_ref().map_or_default(|s| (s.addr.clone(), s.fingerprint.clone()));
        let checked = self.server_check.as_ref().and_then(|rx| rx.try_recv().ok());
        if checked.is_some() {
            self.server_check = None;
        }
        let checking = self.server_check.is_some();
        if checking {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let edit = self.server_edit.get_or_insert_with(|| saved.clone());
        let (mut save, mut clear, mut typed) = (false, false, false);
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::GLOBE, tr("Server", "Server"));
            egui::Grid::new("server_edit").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                widgets::hint(ui, tr("Address", "Ünvan"));
                typed |= ui
                    .add(TextEdit::singleline(&mut edit.0).hint_text("rider.example.com").desired_width(f32::INFINITY))
                    .changed();
                ui.end_row();
            });
            egui::CollapsingHeader::new(tr("Advanced", "Ətraflı")).id_salt("server_advanced").show(ui, |ui| {
                widgets::hint(
                    ui,
                    tr(
                        "Fingerprint: only for a server without a name (an IP address). Otherwise it is checked over HTTPS by itself.",
                        "Barmaq izi: yalnız adı olmayan server (IP ünvanı) üçün. Əks halda o, HTTPS ilə avtomatik yoxlanılır.",
                    ),
                );
                typed |= ui
                    .add(
                        TextEdit::singleline(&mut edit.1)
                            .hint_text("SHA-256")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
            });
            if typed {
                self.server_msg = None;
            }
            ui.horizontal(|ui| {
                let changed = (edit.0.trim(), edit.1.trim()) != (saved.0.as_str(), saved.1.as_str());
                let ok = changed && !edit.0.trim().is_empty() && !checking;
                save = widgets::primary_busy(ui, ok, checking, tr("Save", "Yadda saxla")).clicked();
                if self.cfg.server.is_some() {
                    clear = ui.add_enabled(!checking, egui::Button::new(tr("Remove", "Sil"))).clicked();
                }
                if checking {
                    widgets::hint(ui, tr("Checking the server…", "Server yoxlanılır…"));
                }
            });
            widgets::hint(
                ui,
                tr(
                    "Type the server's name, e.g. rider.example.com. Without a server, only computers on this network can be reached.",
                    "Serverin adını yazın, məsələn, rider.example.com. Server olmadan yalnız bu şəbəkədəki kompüterlərə qoşulmaq mümkündür.",
                ),
            );
            match &self.server_msg {
                Some(Ok(m)) => widgets::success(ui, m),
                Some(Err(e)) => widgets::error(ui, e),
                None => {}
            }
        });
        if save {
            let (addr, fp) = (edit.0.trim().to_owned(), edit.1.trim().to_owned());
            let typed = (!fp.is_empty() && fp != saved.1).then_some(fp);
            self.server_check = Some(check_server(ui.ctx().clone(), addr, typed));
            self.server_msg = None;
            return;
        }
        let (result, verified) = match (clear, checked) {
            (true, _) => (Ok(None), false),
            (false, Some((r, verified))) => (r.map(Some), verified),
            (false, None) => return,
        };
        self.server_saved(result, clear, verified);
    }

    fn server_saved(&mut self, result: Result<Option<ServerRef>, String>, clear: bool, verified: bool) {
        let result = result.and_then(|server| {
            let mut cfg = ClientConfig::load().map_err(|e| format!("{e:#}"))?;
            cfg.server.clone_from(&server);
            cfg.save().map_err(|e| format!("{e:#}"))?;
            Ok(server)
        });
        self.server_msg = Some(match result {
            Ok(server) => {
                self.server_edit = Some(server.as_ref().map_or_default(|s| (s.addr.clone(), s.fingerprint.clone())));
                self.cfg.server = server;
                self.cfg_mtime = config_mtime();
                self.server_error = None;
                self.polled = None;
                Ok(if clear {
                    tr("Server removed.", "Server silindi.")
                } else if verified {
                    tr("Saved. The server was verified over HTTPS.", "Yadda saxlanıldı. Server HTTPS ilə yoxlanıldı.")
                } else {
                    tr("Saved.", "Yadda saxlanıldı.")
                }
                .into())
            }
            Err(e) => Err(e),
        });
    }
}

type ServerCheck = (Result<ServerRef, String>, bool);

fn check_server(ctx: egui::Context, addr: String, typed: Option<String>) -> std::sync::mpsc::Receiver<ServerCheck> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let verified = typed.is_none();
        let r = match typed {
            Some(fp) => ServerRef::new(&addr, &fp),
            None => ServerRef::discover(&addr),
        };
        let _ = tx.send((r.map_err(|e| format!("{e:#}")), verified));
        ctx.request_repaint();
    });
    rx
}

struct SessionLine {
    id: u64,
    finished: Option<bool>,
    connected: Option<String>,
    error: String,
}

impl SessionLine {
    fn show(&self, ui: &mut Ui) -> Option<u64> {
        let mut dismiss = None;
        ui.horizontal(|ui| match (self.finished, &self.connected) {
            (None, None) => {
                ui.spinner();
                ui.label(tr("Connecting…", "Qoşulur…"));
            }
            (None, Some(route)) => {
                widgets::dot(ui, Some(GREEN));
                ui.label(trf!(
                    "Connected ({route}). The session is open in its own window.",
                    "Qoşulub ({route}). Seans ayrıca pəncərədə açıqdır."
                ));
            }
            (Some(_), _) => {
                widgets::error(ui, &self.error);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button(tr("Dismiss", "Bağla")).clicked() {
                        dismiss = Some(self.id);
                    }
                });
            }
        });
        dismiss
    }
}

fn host_summary(ui: &mut Ui, h: &KnownHost, online: Option<bool>) {
    ui.vertical(|ui| {
        ui.label(widgets::title(&h.name, 16.0));
        ui.horizontal(|ui| {
            match online {
                Some(true) => widgets::pill(ui, tr("Online", "Onlayn"), GREEN),
                Some(false) => widgets::pill(ui, tr("Offline", "Oflayn"), GREY),
                None if h.addr.is_empty() => {}
                None => widgets::hint(ui, &h.addr),
            }
            let last = h.last_connected.map_or_else(
                || tr("Never connected", "Heç vaxt qoşulmayıb").to_owned(),
                |t| {
                    let when = widgets::ago(t);
                    trf!("Last connected {when}", "Son qoşulma: {when}")
                },
            );
            widgets::hint(ui, &last);
        });
    });
}
