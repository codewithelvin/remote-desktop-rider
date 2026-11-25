use super::{
    ACCENT, AddDialog, AddStage, ClientApp, Key, QuickDialog, QuickStage, RichText, Support, SupportStage, TextEdit,
    egui, host_side, icon, tr, trf, widgets,
};
use egui::Ui;
use rider_client::PairPrompt;

#[derive(Default)]
struct Choice {
    go: bool,
    close: bool,
}

fn cancel_button(ui: &mut Ui) -> bool {
    let mut close = false;
    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
    close
}

fn busy(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.add(egui::Label::new(text).wrap());
    });
}

fn go_or_cancel(ui: &mut Ui, enabled: bool, label: &str) -> Choice {
    let mut c = Choice::default();
    widgets::dialog_buttons(ui, |ui| {
        c.go = widgets::primary(ui, enabled, label).clicked() || (enabled && ui.input(|i| i.key_pressed(Key::Enter)));
        c.close = ui.button(tr("Cancel", "Ləğv et")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
    });
    c
}

fn code_field(ui: &mut Ui, code: &mut String, hint: &str) {
    let r = ui.add(
        TextEdit::singleline(code).hint_text(hint).font(egui::FontId::monospace(18.0)).desired_width(f32::INFINITY),
    );
    if code.is_empty() {
        r.request_focus();
    }
}

fn add_input(ui: &mut Ui, d: &mut AddDialog, error: Option<&String>) -> Choice {
    ui.label(tr("Code or address shown on the other computer", "Digər kompüterdə göstərilən kod və ya ünvan"));
    code_field(ui, &mut d.target, tr("e.g. SF6-CK6 or 192.168.1.20", "məs., SF6-CK6 və ya 192.168.1.20"));
    ui.label(tr("Name in your list (optional)", "Siyahıdakı adı (istəyə bağlı)"));
    ui.add(
        TextEdit::singleline(&mut d.name)
            .hint_text(tr("e.g. Office PC", "məs., Ofis kompüteri"))
            .desired_width(f32::INFINITY),
    );
    if let Some(e) = error {
        widgets::error(ui, e);
    }
    go_or_cancel(ui, !d.target.trim().is_empty(), tr("Continue", "Davam et"))
}

fn add_confirm(ui: &mut Ui, prompt: &PairPrompt) -> Option<bool> {
    let (host, ip) = (&prompt.host_name, prompt.remote.ip());
    ui.add(
        egui::Label::new(trf!(
            "\"{host}\" ({ip}) shows a verification code:",
            "\"{host}\" ({ip}) təsdiq kodu göstərir:"
        ))
        .wrap(),
    );
    widgets::big_code(ui, &prompt.code);
    widgets::hint(
        ui,
        tr(
            "Only continue if it shows exactly this code. Otherwise someone may be listening in.",
            "Yalnız orada məhz bu kod göstərilirsə davam edin. Əks halda əlaqəni kimsə dinləyə bilər.",
        ),
    );
    let mut reply = None;
    widgets::dialog_buttons(ui, |ui| {
        if widgets::primary(ui, true, tr("Yes, it matches", "Bəli, eynidir")).clicked() {
            reply = Some(true);
        }
        if ui.button(tr("No", "Xeyr")).clicked() {
            reply = Some(false);
        }
    });
    reply
}

fn add_done(ui: &mut Ui, name: &str) -> bool {
    widgets::success(ui, &trf!("\"{name}\" was added.", "\"{name}\" əlavə edildi."));
    widgets::hint(
        ui,
        tr(
            "Connect to it any time from your list, with its PIN.",
            "Ona istənilən vaxt siyahınızdan PIN kodu ilə qoşula bilərsiniz.",
        ),
    );
    let mut close = false;
    widgets::dialog_buttons(ui, |ui| {
        close = widgets::primary(ui, true, tr("Done", "Hazırdır")).clicked() || ui.input(|i| i.key_pressed(Key::Enter));
    });
    close
}

fn add_stage(ui: &mut Ui, d: &mut AddDialog) -> (Choice, Option<bool>) {
    let mut reply = None;
    let choice = match &d.stage {
        AddStage::Input { error } => {
            let error = error.clone();
            add_input(ui, d, error.as_ref())
        }
        AddStage::Connecting => {
            busy(ui, tr("Connecting…", "Qoşulur…"));
            Choice { go: false, close: cancel_button(ui) }
        }
        AddStage::Confirm { prompt, .. } => {
            reply = add_confirm(ui, prompt);
            Choice::default()
        }
        AddStage::Waiting { host } => {
            busy(ui, &trf!("Waiting for someone at \"{host}\" to confirm…", "\"{host}\" tərəfində təsdiq gözlənilir…"));
            Choice { go: false, close: cancel_button(ui) }
        }
        AddStage::Done { name } => Choice { go: false, close: add_done(ui, name) },
    };
    (choice, reply)
}

fn quick_needs_server(ui: &mut Ui) {
    let text = if host_side() {
        tr(
            "Quick connect needs a server: set it in This computer → Manage this computer → Server.",
            "Sürətli qoşulma üçün server lazımdır: onu «Bu kompüter» → «Bu kompüteri idarə et» → «Server» bölməsində təyin edin.",
        )
    } else {
        tr(
            "Quick connect needs a server: set it in Settings → Server.",
            "Sürətli qoşulma üçün server lazımdır: onu «Parametrlər» → «Server» bölməsində təyin edin.",
        )
    };
    widgets::error(ui, text);
}

fn quick_input(ui: &mut Ui, q: &mut QuickDialog, has_server: bool, error: Option<&String>) -> Choice {
    if !has_server {
        quick_needs_server(ui);
    }
    ui.label(tr("One-time code shown on the other computer", "Digər kompüterdə göstərilən birdəfəlik kod"));
    code_field(ui, &mut q.code, tr("e.g. SF6-CK6", "məs., SF6-CK6"));
    widgets::hint(
        ui,
        tr(
            "The person there clicks Get a one-time code under This computer, then allows your connection.",
            "Orada olan şəxs «Bu kompüter» bölməsində «Birdəfəlik kod al» düyməsini klikləyir, sonra qoşulmanıza icazə verir.",
        ),
    );
    if let Some(e) = error {
        widgets::error(ui, e);
    }
    go_or_cancel(ui, has_server && !q.code.trim().is_empty(), tr("Connect", "Qoşul"))
}

fn quick_waiting(ui: &mut Ui, host: &str, code: Option<&String>) -> bool {
    busy(
        ui,
        &trf!(
            "Waiting for the person at \"{host}\" to allow the connection…",
            "\"{host}\" kompüterindəki şəxsin icazəsi gözlənilir…"
        ),
    );
    if let Some(code) = code {
        widgets::hint(
            ui,
            tr("Verification code (they see the same one):", "Təsdiq kodu (qarşı tərəf də eyni kodu görür):"),
        );
        widgets::big_code(ui, code);
    }
    cancel_button(ui)
}

fn quick_connected(ui: &mut Ui, host: &str, route: &str) -> bool {
    widgets::success(
        ui,
        &trf!(
            "Connected to \"{host}\" ({route}). The session is open in its own window.",
            "\"{host}\" kompüterinə qoşuldunuz ({route}). Seans ayrıca pəncərədə açıqdır."
        ),
    );
    widgets::hint(
        ui,
        tr(
            "It ends when you disconnect or the other side clicks its red bar. The code no longer works.",
            "Seans siz bağlantını kəsdikdə və ya qarşı tərəf qırmızı zolağa kliklədikdə bitir. Kod artıq etibarsızdır.",
        ),
    );
    let mut close = false;
    widgets::dialog_buttons(ui, |ui| close = widgets::primary(ui, true, tr("OK", "Oldu")).clicked());
    close
}

fn quick_stage(ui: &mut Ui, q: &mut QuickDialog, has_server: bool) -> Choice {
    match q.stage.clone() {
        QuickStage::Input { error } => quick_input(ui, q, has_server, error.as_ref()),
        QuickStage::Connecting => {
            busy(ui, tr("Connecting…", "Qoşulur…"));
            Choice { go: false, close: cancel_button(ui) }
        }
        QuickStage::Waiting { host, code } => Choice { go: false, close: quick_waiting(ui, &host, code.as_ref()) },
        QuickStage::Connected { host, route } => Choice { go: false, close: quick_connected(ui, &host, &route) },
    }
}

fn support_waiting(ui: &mut Ui, s: &Support) -> bool {
    match &s.code {
        Some(code) => {
            ui.label(tr("Tell this code to the person who is helping you:", "Bu kodu sizə kömək edən şəxsə bildirin:"));
            widgets::big_code(ui, code);
            widgets::hint(
                ui,
                tr(
                    "They enter it under Quick connect. It works once and expires in 10 minutes.",
                    "O, kodu «Sürətli qoşulma» bölməsində daxil edir. Kod bir dəfə işləyir və 10 dəqiqəyə qüvvədən düşür.",
                ),
            );
        }
        None if s.server_error.is_none() => busy(ui, tr("Getting a code from the server…", "Serverdən kod alınır…")),
        None => {}
    }
    if let Some(e) = &s.server_error {
        widgets::error(ui, &format!("{}: {e}", tr("Server", "Server")));
    }
    if let Some(n) = &s.note {
        widgets::hint(ui, n);
    }
    if s.code.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            widgets::hint(ui, tr("Waiting for the connection…", "Qoşulma gözlənilir…"));
        });
    }
    widgets::hint(
        ui,
        tr(
            "In this kind of session, windows running as administrator and UAC prompts can't be controlled.",
            "Bu cür seansda administrator hüquqları ilə işləyən pəncərələri və UAC sorğularını idarə etmək mümkün deyil.",
        ),
    );
    cancel_button(ui)
}

fn support_asking(ui: &mut Ui, s: &mut Support, device: &str, code: &str) -> Option<bool> {
    let text = trf!(
        "\"{device}\" wants to see and control this screen.",
        "\"{device}\" bu ekranı görmək və idarə etmək istəyir."
    );
    ui.add(egui::Label::new(RichText::new(text).strong()).wrap());
    widgets::hint(
        ui,
        tr("Verification code (the helper sees the same one):", "Təsdiq kodu (kömək edən şəxs də eyni kodu görür):"),
    );
    widgets::big_code(ui, code);
    ui.add_space(6.0);
    ui.label(RichText::new(tr("What can they do?", "Nə edə bilər?")).strong());
    ui.add_space(2.0);
    widgets::access_choice(ui, &mut s.access);
    widgets::hint(
        ui,
        tr(
            "Allow only if you know who this is. You can change this or end the session at any time.",
            "Yalnız kim olduğunu bilirsinizsə icazə verin. Bunu istənilən vaxt dəyişə və ya seansı bitirə bilərsiniz.",
        ),
    );
    let mut answer = None;
    widgets::dialog_buttons(ui, |ui| {
        if widgets::primary(ui, true, tr("Allow", "İcazə ver")).clicked() {
            answer = Some(true);
        }
        if ui.button(tr("Decline", "İmtina et")).clicked() {
            answer = Some(false);
        }
    });
    answer
}

#[derive(Default)]
struct SupportChoice {
    close: bool,
    again: bool,
    answer: Option<bool>,
}

fn support_stage(ui: &mut Ui, s: &mut Support) -> SupportChoice {
    let mut c = SupportChoice::default();
    match s.stage.clone() {
        SupportStage::Waiting => c.close = support_waiting(ui, s),
        SupportStage::Asking { device, code } => c.answer = support_asking(ui, s, &device, &code),
        SupportStage::Active { .. } => {}
        SupportStage::Ended => {
            widgets::success(
                ui,
                tr("The session has ended. The code no longer works.", "Seans başa çatdı. Kod artıq etibarsızdır."),
            );
            widgets::dialog_buttons(ui, |ui| c.close = widgets::primary(ui, true, tr("Close", "Bağla")).clicked());
        }
        SupportStage::Expired => {
            widgets::hint(ui, tr("The code expired without being used.", "Kod istifadə edilmədən qüvvədən düşdü."));
            widgets::dialog_buttons(ui, |ui| {
                c.again = widgets::primary(ui, true, tr("Get a new code", "Yeni kod al")).clicked();
                c.close = ui.button(tr("Close", "Bağla")).clicked();
            });
        }
        SupportStage::Failed(e) => {
            widgets::error(ui, &e);
            c.close = close_button(ui);
        }
    }
    c
}

fn close_button(ui: &mut Ui) -> bool {
    let mut close = false;
    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Close", "Bağla")).clicked());
    close
}

impl ClientApp {
    pub(super) fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.add else { return };
        let mut choice = Choice::default();
        let mut reply = None;
        let shape = (std::mem::discriminant(&d.stage), matches!(&d.stage, AddStage::Input { error: Some(_) }));
        widgets::modal(ctx, "add-computer", shape).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(widgets::title(tr("Add a computer", "Kompüter əlavə et"), 19.0));
            ui.add_space(4.0);
            (choice, reply) = add_stage(ui, d);
        });
        if let Some(yes) = reply
            && let AddStage::Confirm { prompt, answer } =
                std::mem::replace(&mut d.stage, AddStage::Input { error: None })
        {
            let _ = answer.send(yes);
            d.stage = if yes { AddStage::Waiting { host: prompt.host_name } } else { AddStage::Connecting };
        }
        if choice.close {
            self.add = None;
        } else if choice.go {
            self.start_pairing();
        }
    }

    pub(super) fn connect_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.connect else { return };
        let mut choice = Choice::default();
        widgets::modal(ctx, "connect", d.error.is_some()).show(ctx, |ui| {
            ui.set_width(380.0);
            let name = &d.name;
            ui.label(widgets::title(trf!("Connect to \"{name}\"", "\"{name}\" kompüterinə qoşulma"), 19.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::KEY).color(ACCENT));
                ui.label(tr("PIN of that computer", "Həmin kompüterin PIN kodu"));
            });
            let r = ui.add(TextEdit::singleline(&mut d.pin).password(true).desired_width(f32::INFINITY));
            r.request_focus();
            if let Some(e) = &d.error {
                widgets::error(ui, e);
            }
            ui.add_space(2.0);
            ui.checkbox(&mut d.remember, tr("Remember PIN on this device", "PIN kodu bu cihazda yadda saxla"));
            widgets::hint(
                ui,
                tr(
                    "Next time Connect opens the session directly. Anyone who can sign in to your account on this device could too.",
                    "Növbəti dəfə «Qoşul» seansı birbaşa açacaq. Bu cihazda hesabınıza daxil ola bilən hər kəs də bunu edə bilər.",
                ),
            );
            choice = go_or_cancel(ui, !d.pin.is_empty(), tr("Connect", "Qoşul"));
        });
        if choice.go {
            if let Some(d) = self.connect.take() {
                self.start_session(&d, false);
            }
        } else if choice.close {
            self.connect = None;
        }
    }

    pub(super) fn quick_dialog(&mut self, ctx: &egui::Context) {
        let has_server = self.cfg.server.is_some();
        let Some(q) = &mut self.quick else { return };
        let mut choice = Choice::default();
        let shape = (std::mem::discriminant(&q.stage), matches!(&q.stage, QuickStage::Input { error: Some(_) }));
        widgets::modal(ctx, "quick-connect", shape).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(widgets::title(tr("Quick connect", "Sürətli qoşulma"), 19.0));
            ui.add_space(4.0);
            choice = quick_stage(ui, q, has_server);
        });
        if choice.go {
            self.start_quick();
        } else if choice.close
            && let Some(mut q) = self.quick.take()
            && !matches!(q.stage, QuickStage::Connected { .. })
        {
            q.stop();
        }
    }

    pub(super) fn support_dialog(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.support else { return };
        if matches!(s.stage, SupportStage::Active { .. }) {
            return;
        }
        let mut c = SupportChoice::default();
        widgets::modal(ctx, "support", std::mem::discriminant(&s.stage)).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(widgets::title(tr("One-time code", "Birdəfəlik kod"), 19.0));
            ui.add_space(4.0);
            c = support_stage(ui, s);
        });
        if let Some(yes) = c.answer {
            let line = if yes { format!("y {}", s.access.code()) } else { "n".into() };
            s.send(&line);
        }
        if c.close || c.again {
            s.send("quit");
            self.support = None;
        }
        if c.again {
            self.start_support();
        }
    }
}
