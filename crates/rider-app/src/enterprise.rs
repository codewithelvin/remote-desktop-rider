use crate::widgets::{self, ACCENT, AMBER, GREEN, GREY, icon};
use eframe::egui::{self, Align, Key, Layout, RichText, ScrollArea, TextEdit, Ui, vec2};
use rider_core::company::{Attention, Computer, DirEdit, Directory, JoinToken, Place, Whereabouts};
use rider_core::i18n::tr;
use rider_core::identity::{Fingerprint, Identity, fp_from_hex};
use rider_core::rendezvous::ServerRef;
use rider_core::trf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(10);
const TILE_MIN_W: f32 = 210.0;
const TILE_H: f32 = 72.0;
const TILE_GAP: f32 = 10.0;
const ROW_H: f32 = 30.0;
const TREE_W: f32 = 236.0;

pub struct Env<'a> {
    pub rt: &'a tokio::runtime::Handle,
    pub identity: Option<&'a Arc<Identity>>,
    pub server: Option<&'a ServerRef>,
    pub admin: Option<&'a rider_client::config::EnterpriseAdmin>,
}

pub enum Outcome {
    SignedIn(rider_client::config::EnterpriseAdmin),
    SignedOut,
    OpenSettings,
}

enum Reply {
    Claimed(Result<(String, String), String>),
    Directory(Result<(Directory, Option<rider_core::rendezvous::Places>), String>),
    Session(Fingerprint, Session),
}

#[derive(Clone)]
enum Session {
    Waiting,
    Connected,
    Failed(String),
    Ended,
}

struct Dragged(Vec<Fingerprint>);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Selected {
    Attention,
    #[default]
    Unassigned,
    Room(u32),
}

#[derive(Clone, Copy)]
enum Count {
    Online(usize, usize),
    Flagged(usize),
}

#[derive(Clone)]
enum Prompt {
    NewFloor,
    NewRoom(u32),
    Rename(u32),
}

pub struct Enterprise {
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    ctx: egui::Context,
    dir: Option<Directory>,
    places: std::collections::HashMap<Fingerprint, Whereabouts>,
    flagged: std::collections::HashMap<Fingerprint, Attention>,
    error: Option<String>,
    busy: bool,
    fetched: Option<Instant>,
    selected: Selected,
    search: String,
    code: String,
    prompt: Option<(Prompt, String)>,
    notice: Option<String>,
    sessions: std::collections::HashMap<Fingerprint, Session>,
    waiting: std::collections::HashMap<Fingerprint, std::process::ChildStdin>,
    picked: std::collections::HashSet<Fingerprint>,
    anchor: Option<Fingerprint>,
}

impl Enterprise {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            ctx: ctx.clone(),
            dir: None,
            places: std::collections::HashMap::new(),
            flagged: std::collections::HashMap::new(),
            error: None,
            busy: false,
            fetched: None,
            selected: Selected::default(),
            search: String::new(),
            code: String::new(),
            prompt: None,
            notice: None,
            sessions: std::collections::HashMap::new(),
            waiting: std::collections::HashMap::new(),
            picked: std::collections::HashSet::new(),
            anchor: None,
        }
    }

    pub fn preview_state(&mut self, state: &str) {
        self.preview();
        let Some(dir) = &self.dir else { return };
        let fp = |i: usize| dir.computers[i].fingerprint;
        match state {
            "selected" => {
                self.picked = [fp(0), fp(1), fp(3)].into();
                self.sessions.insert(fp(0), Session::Connected);
                self.sessions.insert(
                    fp(2),
                    Session::Failed(
                        tr("the person at that computer declined", "həmin kompüterdəki şəxs imtina etdi").into(),
                    ),
                );
                self.sessions.insert(fp(3), Session::Waiting);
            }
            "empty" => self.selected = Selected::Room(5),
            "attention" => self.selected = Selected::Attention,
            "prompt" => self.prompt = Some((Prompt::NewRoom(1), String::new())),
            _ => {}
        }
    }

    pub fn preview_many(&mut self, n: u32) {
        self.preview();
        let Some(dir) = &mut self.dir else { return };
        let template = dir.computers[0].clone();
        dir.computers = (0..n)
            .map(|i| {
                let mut c = template.clone();
                c.fingerprint[..4].copy_from_slice(&i.to_le_bytes());
                c.info.name = format!("PC-{i:04}");
                c.info.user = (i % 3 != 0).then(|| format!("User {i}"));
                c.online = i % 5 != 0;
                c.room = None;
                c
            })
            .collect();
        self.flag_computers();
    }

    pub fn preview(&mut self) {
        use rider_core::company::{Floor, HostInfo, Room};
        let now = rider_core::now_us() / 1_000_000;
        let pc = |n: u8, name: &str, user: Option<&str>, room, online, locked| Computer {
            fingerprint: [n; 32],
            info: HostInfo {
                name: name.into(),
                user: user.map(Into::into),
                locked,
                version: if n == 4 { "0.8.2" } else { env!("CARGO_PKG_VERSION") }.into(),
                unattended: n == 9,
            },
            room,
            unattended_on: n == 9,
            online,
            last_seen: now - if n == 3 { 19 * 86_400 } else { 7200 },
            joined: now - 86_400,
        };
        let room = |id, name: &str| Room { id, name: name.into() };
        self.dir = Some(Directory {
            company: "Northwind".into(),
            floors: vec![
                Floor { id: 1, name: "Floor 3".into(), rooms: vec![room(2, "Room 301"), room(3, "Room 310")] },
                Floor { id: 4, name: "Floor 4".into(), rooms: vec![room(5, "Room 402")] },
            ],
            computers: vec![
                pc(1, "PC-310-01", Some("Alex K."), Some(3), true, false),
                pc(2, "PC-310-02", Some("Maria L."), Some(3), true, true),
                pc(3, "PC-310-03", None, Some(3), false, false),
                pc(4, "PC-310-04", None, Some(3), true, false),
                pc(9, "SRV-310-FILES", None, Some(3), true, false),
                pc(5, "PC-301-01", Some("Sam T."), Some(2), true, false),
                pc(6, "LAPTOP-NEW-17", Some("Priya N."), None, true, false),
            ],
        });
        let gp = |place| Whereabouts { place, via: "PANGP Virtual Ethernet Adapter".into() };
        self.places = [([1; 32], gp(Place::Remote)), ([6; 32], gp(Place::Remote)), ([2; 32], gp(Place::Office))].into();
        self.selected = Selected::Room(3);
        self.fetched = Some(Instant::now() + Duration::from_hours(24));
        self.flag_computers();
    }

    fn flag_computers(&mut self) {
        self.flagged.clear();
        let Some(dir) = &self.dir else { return };
        let now = rider_core::now_us() / 1_000_000;
        let newest = dir.newest_release(env!("CARGO_PKG_VERSION"));
        self.flagged.extend(dir.computers.iter().filter_map(|c| Some((c.fingerprint, c.attention(now, newest)?))));
    }

    fn request(&mut self, env: &Env<'_>, edit: Option<DirEdit>) {
        let (Some(id), Some(server)) = (env.identity.cloned(), env.server.cloned()) else { return };
        self.busy = true;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        env.rt.spawn(async move {
            let r = match edit {
                Some(edit) => rider_core::rendezvous::directory(&id, &server, Some(edit)).await.map(|d| (d, None)),
                None => rider_core::rendezvous::directory_and_places(&id, &server).await,
            };
            let r = r.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Reply::Directory(r));
            ctx.request_repaint();
        });
    }

    fn place(&self, c: &Computer) -> Option<&Whereabouts> {
        self.places.get(&c.fingerprint).filter(|_| c.online)
    }

    fn remote_count(&self) -> usize {
        let Some(dir) = &self.dir else { return 0 };
        dir.computers.iter().filter(|c| self.place(c).is_some_and(|w| w.place == Place::Remote)).count()
    }

    fn claim(&mut self, env: &Env<'_>) {
        let (Some(id), Some(server)) = (env.identity.cloned(), env.server.cloned()) else { return };
        self.busy = true;
        self.error = None;
        let (tx, ctx, code) = (self.tx.clone(), self.ctx.clone(), self.code.clone());
        env.rt.spawn(async move {
            let r = rider_core::rendezvous::admin_claim(&id, &server, &code).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Reply::Claimed(r));
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut Ui, env: &Env<'_>) -> Option<Outcome> {
        let mut outcome = None;
        while let Ok(reply) = self.rx.try_recv() {
            if !matches!(reply, Reply::Session(..)) {
                self.busy = false;
            }
            match reply {
                Reply::Claimed(Ok((company, join_secret))) => {
                    self.code.clear();
                    self.dir = None;
                    self.fetched = None;
                    outcome = Some(Outcome::SignedIn(rider_client::config::EnterpriseAdmin { company, join_secret }));
                }
                Reply::Claimed(Err(e)) => self.error = Some(e),
                Reply::Directory(Ok((d, places))) => {
                    if let Some(places) = places {
                        self.places = places.into_iter().collect();
                    }
                    if let Selected::Room(r) = self.selected
                        && d.room(r).is_none()
                    {
                        self.selected = Selected::Unassigned;
                    }
                    self.dir = Some(d);
                    self.flag_computers();
                    self.error = None;
                    self.fetched = Some(Instant::now());
                }
                Reply::Session(fp, Session::Ended) => {
                    self.sessions.remove(&fp);
                    self.waiting.remove(&fp);
                }
                Reply::Session(fp, Session::Failed(_)) if !self.sessions.contains_key(&fp) => {}
                Reply::Session(fp, state) => {
                    if !matches!(state, Session::Waiting) {
                        self.waiting.remove(&fp);
                    }
                    self.sessions.insert(fp, state);
                }
                Reply::Directory(Err(e)) => {
                    self.error = Some(e);
                    self.fetched = Some(Instant::now());
                }
            }
        }
        if outcome.is_some() {
            return outcome;
        }
        match env.admin {
            None => outcome = self.sign_in(ui, env),
            Some(admin) => {
                if !self.busy && self.fetched.is_none_or(|t| t.elapsed() >= REFRESH) {
                    self.request(env, None);
                }
                ui.ctx().request_repaint_after(REFRESH);
                outcome = self.console(ui, env, admin);
            }
        }
        outcome
    }

    fn sign_in(&mut self, ui: &mut Ui, env: &Env<'_>) -> Option<Outcome> {
        let mut outcome = None;
        widgets::page_header(ui, tr("Enterprise", "Müəssisə"), "", |_| {});
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::COMPANY, tr("Enterprise mode", "Müəssisə rejimi"));
            widgets::hint(
                ui,
                tr(
                    "Look after all of your company's computers from here, by floor and room. Employees accept or decline each connection. Sign in once with the setup code from your company's server.",
                    "Şirkətinizin bütün kompüterlərinə buradan, mərtəbə və otaq üzrə baxın. Hər qoşulmanı işçi qəbul və ya rədd edir. Şirkət serverinizin verdiyi quraşdırma kodu ilə bir dəfə daxil olun.",
                ),
            );
            ui.add_space(8.0);
            let Some(server) = env.server else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::INFO).color(ACCENT));
                    ui.label(tr(
                        "First set your company's server in Settings.",
                        "Əvvəlcə Parametrlərdə şirkətinizin serverini təyin edin.",
                    ));
                });
                ui.add_space(6.0);
                if widgets::primary(ui, true, tr("Open Settings", "Parametrləri aç")).clicked() {
                    outcome = Some(Outcome::OpenSettings);
                }
                return;
            };
            widgets::hint(ui, &trf!("Server: {}", "Server: {}", server.addr));
            ui.add_space(4.0);
            let mut go = false;
            ui.horizontal(|ui| {
                let edit = ui.add(
                    TextEdit::singleline(&mut self.code)
                        .hint_text("XXXX-XXXX-XXXX")
                        .desired_width(200.0)
                        .font(egui::TextStyle::Monospace),
                );
                go = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let ready = !self.busy && self.code.trim().len() >= 12;
                go |= widgets::primary_busy(ui, ready, self.busy, tr("Sign in", "Daxil ol")).clicked();
                go &= ready;
            });
            widgets::hint(
                ui,
                tr(
                    "On the server: rider-server company init --name \"Your company\" (or company setup-code).",
                    "Serverdə: rider-server company init --name \"Şirkətiniz\" (və ya company setup-code).",
                ),
            );
            if go {
                self.claim(env);
            }
            if let Some(e) = &self.error {
                widgets::error(ui, e);
            }
        });
        outcome
    }

    fn console(
        &mut self,
        ui: &mut Ui,
        env: &Env<'_>,
        admin: &rider_client::config::EnterpriseAdmin,
    ) -> Option<Outcome> {
        let mut outcome = None;
        let mut edits: Vec<DirEdit> = Vec::new();
        let (online, total) =
            self.dir.as_ref().map_or((0, 0), |d| (d.computers.iter().filter(|c| c.online).count(), d.computers.len()));
        let mut subtitle = trf!("{} of {} computers online", "{} / {} kompüter onlayndır", online, total);
        let remote = self.remote_count();
        if remote > 0 {
            subtitle += &trf!(", {} remote", ", {} uzaqdan", remote);
        }
        widgets::page_header(ui, &admin.company, &subtitle, |ui| {
            if ui
                .button(tr("Sign out", "Çıxış"))
                .on_hover_text(tr(
                    "This app stops being the admin here. The company and its computers stay on the server.",
                    "Bu proqram burada admin olmaqdan çıxır. Şirkət və kompüterləri serverdə qalır.",
                ))
                .clicked()
            {
                outcome = Some(Outcome::SignedOut);
            }
            if ui
                .button(format!("{} {}", icon::COPY, tr("Join token", "Qoşulma tokeni")))
                .on_hover_text(tr(
                    "For the company installer: computers installed with it join this list by themselves.",
                    "Şirkət quraşdırıcısı üçün: onunla quraşdırılan kompüterlər bu siyahıya özləri qoşulur.",
                ))
                .clicked()
            {
                match join_token(env, admin) {
                    Ok(t) => {
                        ui.ctx().copy_text(t);
                        self.notice = Some(tr("Join token copied.", "Qoşulma tokeni kopyalandı.").into());
                    }
                    Err(e) => self.notice = Some(e),
                }
            }
            if self.busy {
                widgets::icon_spinner(ui);
            } else if widgets::icon_button(ui, icon::REFRESH, tr("Refresh", "Yenilə")).clicked() {
                self.request(env, None);
            }
        });
        if let Some(e) = &self.error {
            widgets::error(ui, e);
        }
        if let Some(n) = &self.notice {
            widgets::success(ui, n);
        }
        let Some(dir) = self.dir.clone() else {
            if self.error.is_none() {
                ui.spinner();
            }
            return outcome;
        };

        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(TREE_W, height), Layout::top_down(Align::Min), |ui| {
                ui.set_width(TREE_W);
                ui.add(
                    TextEdit::singleline(&mut self.search)
                        .hint_text(format!("{} {}", icon::SEARCH, tr("Computer or user", "Kompüter və ya istifadəçi")))
                        .desired_width(TREE_W - 8.0)
                        .margin(egui::Margin::symmetric(8, 5)),
                );
                ui.add_space(8.0);
                ScrollArea::vertical().id_salt("tree").auto_shrink([false, false]).show(ui, |ui| {
                    self.tree(ui, &dir, &mut edits);
                });
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(10.0);
            ui.vertical(|ui| self.tiles(ui, &dir, &mut edits));
        });
        self.prompt_dialog(ui.ctx(), &dir, &mut edits);
        if let Some(edit) = edits.into_iter().next() {
            self.notice = None;
            self.request(env, Some(edit));
        }
        outcome
    }

    fn row(
        ui: &mut Ui,
        indent: f32,
        text: &str,
        count: Count,
        selected: bool,
        strong: bool,
        drop: bool,
    ) -> egui::Response {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), egui::Sense::click());
        let dark = ui.visuals().dark_mode;
        let dragging = drop && r.dnd_hover_payload::<Dragged>().is_some();
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 6.0, ACCENT.linear_multiply(if dark { 0.25 } else { 0.13 }));
            let bar = egui::Rect::from_center_size(rect.left_center() + vec2(1.5, 0.0), vec2(3.0, 16.0));
            p.rect_filled(bar, 2.0, ACCENT);
        } else if r.hovered() {
            p.rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        if dragging {
            p.rect_stroke(rect, 6.0, (1.5, ACCENT), egui::StrokeKind::Inside);
        }
        let font = egui::FontId::new(
            13.5,
            if strong || selected { widgets::semibold() } else { egui::FontFamily::Proportional },
        );
        let text_color = if selected { ACCENT } else { ui.visuals().text_color() };
        let x = rect.left() + 10.0 + indent;
        let count_w = 46.0;
        let galley = ui.painter().layout(text.to_owned(), font, text_color, rect.right() - count_w - x);
        p.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, text_color);
        let weak = ui.visuals().weak_text_color();
        let (on, all) = match count {
            Count::Online(on, all) => (on, all),
            Count::Flagged(n) => {
                let color = if n > 0 { AMBER } else { weak };
                let f = egui::FontId::proportional(12.5);
                p.text(rect.right_center() - vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, n.to_string(), f, color);
                return r.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
        };
        if all == 0 {
            p.text(
                rect.right_center() - vec2(10.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                tr("empty", "boş"),
                egui::FontId::proportional(12.0),
                weak,
            );
        } else {
            let all_text = format!("/{all}");
            let f = egui::FontId::proportional(12.5);
            let right =
                p.text(rect.right_center() - vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, &all_text, f.clone(), weak);
            p.text(
                right.left_center(),
                egui::Align2::RIGHT_CENTER,
                on.to_string(),
                f,
                if on > 0 { GREEN } else { weak },
            );
        }
        r.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn target_row(
        &mut self,
        ui: &mut Ui,
        target: Selected,
        indent: f32,
        text: &str,
        count: Count,
        edits: &mut Vec<DirEdit>,
    ) -> egui::Response {
        let droppable = target != Selected::Attention;
        let selected = self.selected == target && self.search.is_empty();
        let r = Self::row(ui, indent, text, count, selected, false, droppable);
        if r.clicked() {
            if self.selected != target {
                self.picked.clear();
            }
            self.selected = target;
            self.search.clear();
        }
        if let Some(d) = r.dnd_release_payload::<Dragged>().filter(|_| droppable) {
            let room = match target {
                Selected::Attention | Selected::Unassigned => None,
                Selected::Room(id) => Some(id),
            };
            edits.push(DirEdit::Place { computers: d.0.clone(), room });
            self.picked.clear();
        }
        r
    }

    fn add_link(ui: &mut Ui, indent: f32, text: &str) -> bool {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H - 4.0), egui::Sense::click());
        let color = if r.hovered() { ACCENT } else { ui.visuals().weak_text_color() };
        let p = ui.painter();
        let at = rect.left_center() + vec2(10.0 + indent, 0.0);
        let plus = p.text(at, egui::Align2::LEFT_CENTER, icon::ADD, egui::FontId::proportional(11.0), color);
        p.text(
            plus.right_center() + vec2(7.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(13.0),
            color,
        );
        r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }

    fn tree(&mut self, ui: &mut Ui, dir: &Directory, edits: &mut Vec<DirEdit>) {
        ui.spacing_mut().item_spacing.y = 2.0;
        let mut per_room: std::collections::HashMap<Option<u32>, (usize, usize)> = std::collections::HashMap::default();
        for c in &dir.computers {
            let e = per_room.entry(c.room).or_default();
            e.0 += usize::from(c.online);
            e.1 += 1;
        }
        let count = |room: Option<u32>| {
            let (on, all) = per_room.get(&room).copied().unwrap_or_default();
            Count::Online(on, all)
        };
        let flagged = Count::Flagged(self.flagged.len());
        self.target_row(ui, Selected::Attention, 0.0, tr("Needs attention", "Diqqət tələb edir"), flagged, edits);
        self.target_row(ui, Selected::Unassigned, 0.0, tr("Unassigned", "Təyin edilməyib"), count(None), edits);
        ui.add_space(8.0);
        for floor in &dir.floors {
            let id = ui.make_persistent_id(("floor", floor.id));
            let open = ui.data_mut(|d| *d.get_persisted_mut_or(id, true));
            let (on, all) = floor
                .rooms
                .iter()
                .map(|r| per_room.get(&Some(r.id)).copied().unwrap_or_default())
                .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
            let r = Self::row(ui, 16.0, &floor.name, Count::Online(on, all), false, true, false);
            chevron(ui.painter(), r.rect.left_center() + vec2(16.0, 0.0), open, ui.visuals().weak_text_color());
            if r.clicked() {
                ui.data_mut(|d| d.insert_persisted(id, !open));
            }
            r.context_menu(|ui| {
                if ui.button(tr("Add a room…", "Otaq əlavə et…")).clicked() {
                    self.prompt = Some((Prompt::NewRoom(floor.id), String::new()));
                    ui.close();
                }
                if ui.button(tr("Rename…", "Adını dəyiş…")).clicked() {
                    self.prompt = Some((Prompt::Rename(floor.id), floor.name.clone()));
                    ui.close();
                }
                if ui.button(tr("Delete (floors without rooms)", "Sil (otaqsız mərtəbələr)")).clicked() {
                    edits.push(DirEdit::Delete { id: floor.id });
                    ui.close();
                }
            });
            if !open {
                continue;
            }
            for room in &floor.rooms {
                let r = self.target_row(ui, Selected::Room(room.id), 30.0, &room.name, count(Some(room.id)), edits);
                r.context_menu(|ui| {
                    if ui.button(tr("Rename…", "Adını dəyiş…")).clicked() {
                        self.prompt = Some((Prompt::Rename(room.id), room.name.clone()));
                        ui.close();
                    }
                    if ui.button(tr("Delete (empty rooms)", "Sil (boş otaqlar)")).clicked() {
                        edits.push(DirEdit::Delete { id: room.id });
                        ui.close();
                    }
                });
            }
            if Self::add_link(ui, 30.0, tr("Add room", "Otaq əlavə et")) {
                self.prompt = Some((Prompt::NewRoom(floor.id), String::new()));
            }
            ui.add_space(6.0);
        }
        if Self::add_link(ui, 0.0, tr("Add floor", "Mərtəbə əlavə et")) {
            self.prompt = Some((Prompt::NewFloor, String::new()));
        }
    }

    fn shown<'d>(&self, dir: &'d Directory, needle: &str) -> (String, Vec<&'d Computer>) {
        if !needle.is_empty() {
            let hits = dir
                .computers
                .iter()
                .filter(|c| {
                    c.info.name.to_lowercase().contains(needle)
                        || c.info.user.as_deref().is_some_and(|u| u.to_lowercase().contains(needle))
                })
                .collect();
            return (trf!("Search: {}", "Axtarış: {}", self.search.trim()), hits);
        }
        match self.selected {
            Selected::Attention => (
                tr("Needs attention", "Diqqət tələb edir").into(),
                dir.computers.iter().filter(|c| self.flagged.contains_key(&c.fingerprint)).collect(),
            ),
            Selected::Unassigned => (tr("Unassigned", "Təyin edilməyib").into(), dir.in_room(None).collect()),
            Selected::Room(id) => {
                let name = dir.room(id).map_or_else(String::new, |(f, r)| format!("{} › {}", f.name, r.name));
                (name, dir.in_room(Some(id)).collect())
            }
        }
    }

    fn picked_in(&self, list: &[&Computer]) -> Vec<Fingerprint> {
        list.iter().map(|c| c.fingerprint).filter(|fp| self.picked.contains(fp)).collect()
    }

    fn selection_keys(&mut self, ui: &Ui, list: &[&Computer]) {
        if !self.picked.is_empty() {
            let shown: std::collections::HashSet<_> = list.iter().map(|c| c.fingerprint).collect();
            self.picked.retain(|fp| shown.contains(fp));
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing && ui.input(|i| i.modifiers.command && i.key_pressed(Key::A)) {
            self.picked = list.iter().map(|c| c.fingerprint).collect();
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.picked.clear();
        }
    }

    fn tiles_header(
        &mut self,
        ui: &mut Ui,
        title: &str,
        list: &[&Computer],
        dir: &Directory,
        edits: &mut Vec<DirEdit>,
    ) {
        ui.horizontal(|ui| {
            ui.label(widgets::title(title, 15.0));
            if !list.is_empty() {
                let on = list.iter().filter(|c| c.online).count();
                let text = trf!("{} of {} online", "{} / {} onlayn", on, list.len());
                widgets::pill(ui, &text, if on > 0 { GREEN } else { GREY });
            }
            if self.picked.is_empty() {
                return;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(tr("Clear", "Təmizlə")).clicked() {
                    self.picked.clear();
                }
                let picked = self.picked_in(list);
                ui.menu_button(format!("{} ▾", tr("Move to", "Köçür")), |ui| {
                    if let Some(MoveTo(room)) = move_menu(ui, dir) {
                        edits.push(DirEdit::Place { computers: picked.clone(), room });
                        self.picked.clear();
                    }
                });
                ui.label(RichText::new(trf!("{} selected", "{} seçilib", self.picked.len())).color(ACCENT));
            });
        });
    }

    fn empty_tiles(&mut self, ui: &mut Ui, searching: bool, edits: &mut Vec<DirEdit>) {
        let room = match self.selected {
            Selected::Room(id) if !searching => Some(Some(id)),
            Selected::Unassigned if !searching => Some(None),
            _ => None,
        };
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 110.0), egui::Sense::hover());
        let hot = room.is_some() && r.dnd_hover_payload::<Dragged>().is_some();
        let stroke_color = if hot { ACCENT } else { ui.visuals().widgets.noninteractive.bg_stroke.color };
        dashed_rect(ui.painter(), rect, stroke_color);
        if let (Some(room), Some(d)) = (room, r.dnd_release_payload::<Dragged>()) {
            edits.push(DirEdit::Place { computers: d.0.clone(), room });
            self.picked.clear();
        }
        let mut text_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink(16.0))
                .layout(Layout::centered_and_justified(egui::Direction::TopDown)),
        );
        let text = match (self.selected, searching) {
            (_, true) => tr("No computer matches.", "Uyğun kompüter yoxdur."),
            (Selected::Attention, false) => tr(
                "Nothing needs attention: every computer was seen in the last 14 days and runs a current version.",
                "Diqqət tələb edən heç nə yoxdur: bütün kompüterlər son 14 gündə görünüb və aktual versiyadadır.",
            ),
            (Selected::Unassigned, false) => tr(
                "Computers installed with the company installer show up here. Drag them onto a room.",
                "Şirkət quraşdırıcısı ilə quraşdırılan kompüterlər burada görünür. Onları otağa sürükləyin.",
            ),
            (Selected::Room(_), false) => tr(
                "No computers here yet. Drag them here from Unassigned.",
                "Burada hələ kompüter yoxdur. Onları «Təyin edilməyib» bölməsindən bura sürükləyin.",
            ),
        };
        widgets::hint(&mut text_ui, text);
    }

    fn tiles(&mut self, ui: &mut Ui, dir: &Directory, edits: &mut Vec<DirEdit>) {
        let needle = self.search.trim().to_lowercase();
        let (title, list) = self.shown(dir, &needle);
        self.selection_keys(ui, &list);
        self.tiles_header(ui, &title, &list, dir, edits);
        ui.add_space(8.0);
        if list.is_empty() {
            self.empty_tiles(ui, !needle.is_empty(), edits);
            return;
        }
        let avail = ui.available_width() - 14.0;
        let cols = (((avail + TILE_GAP) / (TILE_MIN_W + TILE_GAP)).floor() as usize).max(1);
        let w = ((avail - TILE_GAP * (cols - 1) as f32) / cols as f32).floor();
        let rows = list.len().div_ceil(cols);
        ScrollArea::vertical().id_salt("tiles").auto_shrink([false, false]).show_rows(ui, TILE_H, rows, |ui, range| {
            ui.spacing_mut().item_spacing.y = TILE_GAP;
            for row in range {
                let from = row * cols;
                let to = (from + cols).min(list.len());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = TILE_GAP;
                    for i in from..to {
                        self.tile(ui, dir, &list, i, w, edits);
                    }
                });
            }
        });
    }

    fn tile_status(&self, ui: &Ui, c: &Computer) -> (String, egui::Color32) {
        match self.sessions.get(&c.fingerprint) {
            Some(Session::Waiting) => (tr("Waiting for an answer…", "Cavab gözlənilir…").into(), AMBER),
            Some(Session::Connected) => (tr("In your session", "Sizin seansınızdadır").into(), GREEN),
            Some(Session::Failed(why)) => (why.clone(), ui.visuals().error_fg_color),
            _ => match self.flagged.get(&c.fingerprint).filter(|_| self.selected == Selected::Attention) {
                Some(a) => (attention_text(c, *a), AMBER),
                None => status(c),
            },
        }
    }

    fn is_busy(&self, c: &Computer) -> bool {
        matches!(self.sessions.get(&c.fingerprint), Some(Session::Waiting | Session::Connected))
    }

    fn tile(
        &mut self,
        ui: &mut Ui,
        dir: &Directory,
        list: &[&Computer],
        i: usize,
        width: f32,
        edits: &mut Vec<DirEdit>,
    ) {
        let c = list[i];
        let (status, color) = self.tile_status(ui, c);
        let (rect, r) = ui.allocate_exact_size(vec2(width, TILE_H), egui::Sense::click_and_drag());
        let busy = self.is_busy(c);
        let picked = self.picked.contains(&c.fingerprint);
        let dark = ui.visuals().dark_mode;
        let stroke = if picked {
            egui::Stroke::new(1.5, ACCENT)
        } else if r.hovered() || busy {
            egui::Stroke::new(1.5, if busy { color } else { ACCENT.linear_multiply(0.8) })
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        };
        let fill = if picked { ACCENT.linear_multiply(if dark { 0.18 } else { 0.08 }) } else { widgets::card_fill(ui) };
        ui.painter().rect(rect, 8.0, fill, stroke, egui::StrokeKind::Inside);
        let place = self.place(c);
        tile_body(ui, rect, c, &status, color, place);
        let r = match place.and_then(place_note) {
            Some(note) if !r.dragged() => r.on_hover_text(note),
            _ => r,
        };
        let carried = if picked { self.picked_in(list) } else { vec![c.fingerprint] };
        drag_many(ui, &r, carried);
        if r.clicked() {
            self.tile_clicked(ui, list, i);
        }
        if r.double_clicked() && c.online {
            self.connect(c);
        }
        let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_ui(|ui| tile_tooltip(ui, c));
        let mut connect = false;
        r.context_menu(|ui| connect = self.tile_menu(ui, dir, list, c, edits));
        if connect {
            self.connect(c);
        }
    }

    fn tile_clicked(&mut self, ui: &Ui, list: &[&Computer], i: usize) {
        let c = list[i];
        let m = ui.input(|i| i.modifiers);
        if m.shift
            && let Some(a) = self.anchor.and_then(|a| list.iter().position(|x| x.fingerprint == a))
        {
            if !m.command {
                self.picked.clear();
            }
            for x in &list[a.min(i)..=a.max(i)] {
                self.picked.insert(x.fingerprint);
            }
        } else if m.command {
            if !self.picked.remove(&c.fingerprint) {
                self.picked.insert(c.fingerprint);
            }
            self.anchor = Some(c.fingerprint);
        } else {
            self.picked.clear();
            self.picked.insert(c.fingerprint);
            self.anchor = Some(c.fingerprint);
        }
    }

    fn tile_menu(
        &mut self,
        ui: &mut Ui,
        dir: &Directory,
        list: &[&Computer],
        c: &Computer,
        edits: &mut Vec<DirEdit>,
    ) -> bool {
        let picked = self.picked.contains(&c.fingerprint);
        let group = if picked && self.picked.len() > 1 { self.picked_in(list) } else { vec![c.fingerprint] };
        let mut connect = false;
        if matches!(self.sessions.get(&c.fingerprint), Some(Session::Waiting)) {
            if ui.button(tr("Cancel request", "Sorğunu ləğv et")).clicked() {
                self.cancel(c.fingerprint);
                ui.close();
            }
        } else if ui.add_enabled(c.online && !self.is_busy(c), egui::Button::new(tr("Connect", "Qoşul"))).clicked() {
            connect = true;
            ui.close();
        }
        ui.separator();
        let label = if group.len() > 1 {
            trf!("Move {} computers to", "{} kompüteri köçür", group.len())
        } else {
            tr("Move to", "Köçür").to_owned()
        };
        ui.menu_button(label, |ui| {
            if let Some(MoveTo(room)) = move_menu(ui, dir) {
                edits.push(DirEdit::Place { computers: group.clone(), room });
                self.picked.clear();
            }
        });
        unattended_toggle(ui, c, edits);
        ui.separator();
        if ui.button(tr("Remove from the list", "Siyahıdan sil")).clicked() {
            edits.push(DirEdit::Forget { computer: c.fingerprint });
            ui.close();
        }
        connect
    }

    fn connect(&mut self, c: &Computer) {
        if matches!(self.sessions.get(&c.fingerprint), Some(Session::Waiting | Session::Connected)) {
            return;
        }
        let fp = c.fingerprint;
        let spawned = crate::sys::tool("rider-client.exe")
            .args(["company-connect", &rider_core::identity::fp_hex(&fp)])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                self.sessions.insert(fp, Session::Failed(format!("rider-client: {e}")));
                return;
            }
        };
        self.sessions.insert(fp, Session::Waiting);
        if let Some(stdin) = child.stdin.take() {
            self.waiting.insert(fp, stdin);
        }
        let send = {
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            move |state: Session| {
                let _ = tx.send(Reply::Session(fp, state));
                ctx.request_repaint();
            }
        };
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(out) = child.stdout.take() {
            let (send, connected) = (send.clone(), connected.clone());
            crate::sys::lines(out, move |line| {
                if line.starts_with("Connected to ") {
                    connected.store(true, std::sync::atomic::Ordering::Relaxed);
                    send(Session::Connected);
                }
            });
        }
        let stderr = Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(err) = child.stderr.take() {
            let stderr = stderr.clone();
            crate::sys::lines(err, move |line| {
                let mut s = stderr.lock().unwrap();
                s.push_str(&line);
                s.push('\n');
            });
        }
        std::thread::spawn(move || {
            let ok = child.wait().is_ok_and(|s| s.success());
            std::thread::sleep(Duration::from_millis(200));
            if ok || connected.load(std::sync::atomic::Ordering::Relaxed) {
                send(Session::Ended);
            } else {
                send(Session::Failed(crate::sys::error_text(&stderr.lock().unwrap())));
            }
        });
    }

    fn cancel(&mut self, fp: Fingerprint) {
        use std::io::Write as _;
        if let Some(mut stdin) = self.waiting.remove(&fp) {
            let _ = writeln!(stdin, "cancel").and_then(|()| stdin.flush());
        }
        self.sessions.remove(&fp);
    }

    fn prompt_dialog(&mut self, ctx: &egui::Context, dir: &Directory, edits: &mut Vec<DirEdit>) {
        let Some((what, text)) = &mut self.prompt else { return };
        let title = match what {
            Prompt::NewFloor => tr("New floor", "Yeni mərtəbə").to_owned(),
            Prompt::NewRoom(f) => {
                trf!("New room on {}", "{} üçün yeni otaq", dir.floor(*f).map_or("", |f| f.name.as_str()))
            }
            Prompt::Rename(_) => tr("Rename", "Adını dəyiş").to_owned(),
        };
        let (mut ok, mut cancel) = (false, false);
        widgets::modal(ctx, "enterprise-name", 0).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(widgets::title(&title, 15.0));
            ui.add_space(6.0);
            let hint = match what {
                Prompt::NewFloor => tr("e.g. Floor 3", "məs. 3-cü mərtəbə"),
                _ => tr("e.g. Room 310", "məs. Otaq 310"),
            };
            let edit = ui.add(TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY));
            edit.request_focus();
            ok = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            widgets::dialog_buttons(ui, |ui| {
                ok |= widgets::primary(ui, !text.trim().is_empty(), tr("Save", "Yadda saxla")).clicked();
                cancel = ui.button(tr("Cancel", "Ləğv et")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        let name = text.trim().to_owned();
        if ok && !name.is_empty() {
            edits.push(match *what {
                Prompt::NewFloor => DirEdit::AddFloor { name },
                Prompt::NewRoom(floor) => DirEdit::AddRoom { floor, name },
                Prompt::Rename(id) => DirEdit::Rename { id, name },
            });
            self.prompt = None;
        } else if cancel {
            self.prompt = None;
        }
    }
}

fn tile_body(ui: &Ui, rect: egui::Rect, c: &Computer, status: &str, color: egui::Color32, place: Option<&Whereabouts>) {
    let p = ui.painter();
    let dark = ui.visuals().dark_mode;
    let icon_rect = egui::Rect::from_center_size(rect.left_center() + vec2(30.0, 0.0), vec2(36.0, 36.0));
    p.rect_filled(icon_rect, 8.0, color.linear_multiply(if dark { 0.22 } else { 0.14 }));
    p.text(icon_rect.center(), egui::Align2::CENTER_CENTER, icon::PC, egui::FontId::proportional(18.0), color);
    let x = icon_rect.right() + 12.0;
    let mut right = rect.right() - 12.0;
    if c.unattended() {
        let badge = tr("Unattended", "Nəzarətsiz");
        let galley = p.layout_no_wrap(badge.into(), egui::FontId::proportional(11.0), AMBER);
        let b = egui::Rect::from_min_size(
            egui::pos2(right - galley.size().x - 12.0, rect.top() + 10.0),
            galley.size() + vec2(12.0, 4.0),
        );
        p.rect_filled(b, 8.0, AMBER.linear_multiply(if dark { 0.22 } else { 0.14 }));
        p.galley(b.min + vec2(6.0, 2.0), galley, AMBER);
        right = b.left() - 6.0;
    }
    if place.is_some_and(|w| w.place == Place::Remote) {
        let badge = tr("Remote", "Uzaqdan");
        let galley = p.layout_no_wrap(badge.into(), egui::FontId::proportional(11.0), VIOLET);
        let b = egui::Rect::from_min_size(
            egui::pos2(right - galley.size().x - 12.0, rect.top() + 10.0),
            galley.size() + vec2(12.0, 4.0),
        );
        p.rect_filled(b, 8.0, VIOLET.linear_multiply(if dark { 0.22 } else { 0.14 }));
        p.galley(b.min + vec2(6.0, 2.0), galley, VIOLET);
        right = b.left() - 6.0;
    }
    let text = ui.visuals().text_color();
    let name = p.layout(c.info.name.clone(), egui::FontId::new(14.0, widgets::semibold()), text, (right - x).max(10.0));
    let name = truncate_galley(ui, name, &c.info.name, egui::FontId::new(14.0, widgets::semibold()), text, right - x);
    p.galley(egui::pos2(x, rect.center().y - name.size().y - 1.0), name, text);
    let sub = if color == GREY || color == AMBER || color == GREEN { ui.visuals().weak_text_color() } else { color };
    let line = truncate_galley(
        ui,
        p.layout_no_wrap(status.into(), egui::FontId::proportional(12.5), sub),
        status,
        egui::FontId::proportional(12.5),
        sub,
        rect.right() - 12.0 - x,
    );
    p.galley(egui::pos2(x, rect.center().y + 2.0), line, sub);
}

fn place_note(w: &Whereabouts) -> Option<String> {
    let via = if w.via.is_empty() { String::new() } else { format!(" ({})", w.via) };
    let what = match w.place {
        Place::Office => tr("In the office", "Ofisdədir"),
        Place::Remote => tr("Outside the office, through the VPN", "Ofisdən kənarda, VPN vasitəsilə"),
        Place::Unknown => return None,
    };
    Some(format!("{what}{via}"))
}

const VIOLET: egui::Color32 = egui::Color32::from_rgb(150, 112, 240);

fn truncate_galley(
    ui: &Ui,
    galley: Arc<egui::Galley>,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> Arc<egui::Galley> {
    if galley.size().x <= width && galley.rows.len() <= 1 {
        return galley;
    }
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

fn drag_many(ui: &Ui, r: &egui::Response, carried: Vec<Fingerprint>) {
    let many = carried.len();
    r.dnd_set_drag_payload(Dragged(carried));
    if r.dragged()
        && many > 1
        && let Some(at) = ui.ctx().pointer_interact_pos()
    {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.scope_id().with("dragging-many"), at)
            .show(|ui| ui.label(trf!("{} computers", "{} kompüter", many)));
    }
}

fn tile_tooltip(ui: &mut Ui, c: &Computer) {
    ui.label(RichText::new(&c.info.name).strong());
    ui.label(trf!("Version {}", "Versiya {}", c.info.version));
    if c.unattended() {
        ui.label(tr(
            "Unattended: you connect without anyone accepting.",
            "Nəzarətsiz: heç kim qəbul etmədən qoşulursunuz.",
        ));
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new(tr(
            "Double-click to connect. Drag onto a room, or right-click for more. Ctrl or Shift: pick several.",
            "Qoşulmaq üçün iki dəfə klikləyin. Otağa sürükləyin və ya daha çoxu üçün sağ klikləyin. Ctrl və ya Shift: bir neçəsini seçin.",
        ))
        .weak(),
    );
}

fn unattended_toggle(ui: &mut Ui, c: &Computer, edits: &mut Vec<DirEdit>) {
    let mut on = c.unattended_on;
    let toggle = widgets::checkbox(
        ui,
        c.info.unattended,
        &mut on,
        tr("Unattended (no one asked)", "Nəzarətsiz (heç kimdən soruşulmadan)"),
    );
    let toggle = toggle.on_disabled_hover_text(tr(
        "Only for computers installed as unattended (company installer option).",
        "Yalnız nəzarətsiz kimi quraşdırılmış kompüterlər üçün (şirkət quraşdırıcısının seçimi).",
    ));
    if toggle.changed() {
        edits.push(DirEdit::Unattended { computer: c.fingerprint, on });
        ui.close();
    }
}

struct MoveTo(Option<u32>);

fn move_menu(ui: &mut Ui, dir: &Directory) -> Option<MoveTo> {
    let mut chosen = None;
    if ui.button(tr("Unassigned", "Təyin edilməyib")).clicked() {
        chosen = Some(MoveTo(None));
    }
    for f in &dir.floors {
        for room in &f.rooms {
            if ui.button(format!("{} › {}", f.name, room.name)).clicked() {
                chosen = Some(MoveTo(Some(room.id)));
            }
        }
    }
    if chosen.is_some() {
        ui.close();
    }
    chosen
}

fn chevron(p: &egui::Painter, at: egui::Pos2, open: bool, color: egui::Color32) {
    let s = 3.5;
    let points = if open {
        vec![at + vec2(-s, -s / 2.0), at + vec2(0.0, s / 2.0), at + vec2(s, -s / 2.0)]
    } else {
        vec![at + vec2(-s / 2.0, -s), at + vec2(s / 2.0, 0.0), at + vec2(-s / 2.0, s)]
    };
    p.add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
}

fn dashed_rect(p: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let r = rect.shrink(1.0);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        p.extend(egui::Shape::dashed_line(&[w[0], w[1]], egui::Stroke::new(1.2, color), 6.0, 4.0));
    }
}

fn attention_text(c: &Computer, a: Attention) -> String {
    match a {
        Attention::NotSeen { days } => trf!("Not seen for {} days", "{} gündür görünməyib", days),
        Attention::Outdated { newest } => trf!(
            "Outdated: {}, newest {}.{}",
            "Köhnə versiya: {}, ən yenisi {}.{}",
            c.info.version,
            newest.major,
            newest.minor
        ),
    }
}

fn status(c: &Computer) -> (String, egui::Color32) {
    if !c.online {
        return (trf!("Offline, seen {}", "Oflayn, {}", widgets::ago(c.last_seen)), GREY);
    }
    match (&c.info.user, c.info.locked) {
        (Some(u), false) => (u.clone(), GREEN),
        (Some(u), true) => (trf!("{u} (locked)", "{u} (kilidli)"), AMBER),
        (None, _) => (tr("Nobody signed in", "Heç kim daxil olmayıb").into(), GREEN),
    }
}

fn join_token(env: &Env<'_>, admin: &rider_client::config::EnterpriseAdmin) -> Result<String, String> {
    let server = env.server.ok_or_else(|| tr("No server is set.", "Server təyin edilməyib.").to_owned())?;
    let identity = env.identity.ok_or_else(|| tr("This device has no key.", "Bu cihazın açarı yoxdur.").to_owned())?;
    let server_fingerprint = fp_from_hex(&server.fingerprint).map_err(|e| format!("{e:#}"))?;
    Ok(JoinToken {
        server: server.addr.clone(),
        server_fingerprint,
        admin: identity.fingerprint(),
        secret: admin.join_secret.clone(),
    }
    .encode())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_frames_stay_fast_with_5000_computers() {
        const N: u32 = 30;
        let ctx = egui::Context::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let mut page = Enterprise::new(&ctx);
        page.preview_many(5000);
        page.fetched = Some(Instant::now());
        let admin = rider_client::config::EnterpriseAdmin { company: "Big".into(), join_secret: String::new() };
        let env = Env { rt: &handle, identity: None, server: None, admin: Some(&admin) };
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1300.0, 760.0))),
            ..Default::default()
        };
        let frame = |page: &mut Enterprise| {
            let mut out = ctx.run_ui(input(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    page.show(ui, &env);
                });
            });
            out.textures_delta.clear();
        };
        for _ in 0..3 {
            frame(&mut page);
        }
        let start = Instant::now();
        for _ in 0..N {
            frame(&mut page);
        }
        let per_frame = start.elapsed() / N;
        eprintln!("5000 computers: {per_frame:?} per frame");
        if !cfg!(debug_assertions) {
            assert!(per_frame < Duration::from_millis(12), "a frame took {per_frame:?}");
        }
    }
}
