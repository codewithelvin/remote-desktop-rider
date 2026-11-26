use egui::{Rect, ViewportId, pos2, vec2};
use rider_ui::chat::{Action, Chat};
use rider_ui::soft::{SoftRenderer, Target};
use rider_ui::tunnels::{self, Tunnels};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::event::WindowEvent;
use winit::window::{CursorIcon, Window};

const WIDTH_PT: f64 = 290.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Chat,
    Tunnels,
}

pub struct ChatPanel {
    ctx: egui::Context,
    input: egui_winit::State,
    renderer: SoftRenderer,
    pub chat: Chat,
    open: bool,
    pub ready: bool,
    pixels: Vec<u32>,
    pw: usize,
    ph: usize,
    dirty: bool,
    repaint_at: Option<Instant>,
    cursor: egui::CursorIcon,
    actions: Vec<Action>,
    paths: HashMap<u64, PathBuf>,
    pub tunnels: Tunnels,
    page: Page,
    tunnel_actions: Vec<tunnels::Action>,
}

impl ChatPanel {
    pub fn new(window: &Window, peer: &str) -> Self {
        let ctx = egui::Context::default();
        rider_ui::theme::setup(&ctx);
        let input = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        Self {
            ctx,
            input,
            renderer: SoftRenderer::default(),
            chat: Chat::new(peer),
            open: false,
            ready: false,
            pixels: Vec::new(),
            pw: 0,
            ph: 0,
            dirty: true,
            repaint_at: None,
            cursor: egui::CursorIcon::Default,
            actions: Vec::new(),
            paths: HashMap::new(),
            tunnels: Tunnels::default(),
            page: Page::Chat,
            tunnel_actions: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        if open != self.open {
            self.open = open;
            self.dirty = true;
            if open {
                self.chat.focus();
            }
        }
    }

    pub fn page(&self) -> Page {
        self.page
    }

    pub fn show_page(&mut self, page: Page) {
        if page != self.page {
            self.page = page;
            self.dirty = true;
        }
    }

    pub fn take_tunnel_actions(&mut self) -> Vec<tunnels::Action> {
        std::mem::take(&mut self.tunnel_actions)
    }

    pub fn changed(&mut self) {
        self.dirty = true;
    }

    pub fn width(&self, window_w: usize, scale: f64) -> usize {
        if !self.open {
            return 0;
        }
        ((WIDTH_PT * scale).round() as usize).min(window_w * 2 / 5)
    }

    pub fn contains(&self, x: f64, window_w: usize, scale: f64) -> bool {
        let w = self.width(window_w, scale);
        w > 0 && x >= (window_w - w) as f64
    }

    pub fn typing(&self) -> bool {
        self.open && self.chat.typing(&self.ctx)
    }

    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        if !self.open {
            return false;
        }
        let keys = matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_));
        if keys && !self.typing() {
            return false;
        }
        let r = self.input.on_window_event(window, event);
        if r.repaint {
            self.dirty = true;
        }
        r.repaint
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.repaint_at.filter(|_| self.open)
    }

    pub fn due(&self) -> bool {
        self.open && (self.dirty || self.repaint_at.is_some_and(|t| Instant::now() >= t))
    }

    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    pub fn file_path(&self, id: u64) -> Option<&PathBuf> {
        self.paths.get(&id)
    }

    pub fn set_file_path(&mut self, id: u64, path: PathBuf) {
        self.paths.insert(id, path);
    }

    pub fn cursor(&self) -> CursorIcon {
        match self.cursor {
            egui::CursorIcon::Text => CursorIcon::Text,
            egui::CursorIcon::PointingHand => CursorIcon::Pointer,
            egui::CursorIcon::NotAllowed => CursorIcon::NotAllowed,
            egui::CursorIcon::ResizeVertical => CursorIcon::NsResize,
            _ => CursorIcon::Default,
        }
    }

    pub fn draw(&mut self, window: &Window, buf: &mut [u32], w: usize, h: usize) {
        let pw = self.width(w, window.scale_factor());
        if pw == 0 || h == 0 {
            return;
        }
        let x0 = w - pw;
        if self.due() || (pw, h) != (self.pw, self.ph) {
            self.run(window, x0, pw, h);
        }
        for y in 0..h {
            buf[y * w + x0..(y + 1) * w].copy_from_slice(&self.pixels[y * pw..(y + 1) * pw]);
        }
    }

    fn run(&mut self, window: &Window, x0: usize, pw: usize, h: usize) {
        let ppp = egui_winit::pixels_per_point(&self.ctx, window);
        let mut raw = self.input.take_egui_input(window);
        raw.screen_rect = Some(Rect::from_min_size(pos2(x0 as f32 / ppp, 0.0), vec2(pw as f32 / ppp, h as f32 / ppp)));
        let (chat, tunnels, page) = (&mut self.chat, &mut self.tunnels, self.page);
        let mut actions = Vec::new();
        let mut tunnel_actions = Vec::new();
        let out = self.ctx.run_ui(raw, |ui| {
            match page {
                Page::Chat => actions = chat.show(ui),
                Page::Tunnels => tunnel_actions = tunnels.show(ui),
            }
            let r = ui.max_rect();
            let line = ui.visuals().widgets.noninteractive.bg_stroke;
            ui.painter().vline(r.left() + 0.5, r.y_range(), line);
        });
        self.actions.extend(actions);
        self.tunnel_actions.extend(tunnel_actions);
        let mut platform = out.platform_output;
        self.cursor = std::mem::replace(&mut platform.cursor_icon, egui::CursorIcon::Default);
        self.input.handle_platform_output(window, platform);
        self.renderer.update_textures(&out.textures_delta);
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        self.pixels.clear();
        self.pixels.resize(pw * h, rgb(self.ctx.style_of(self.ctx.theme()).visuals.panel_fill));
        self.renderer.paint(
            &prims,
            out.pixels_per_point,
            &mut Target { buf: &mut self.pixels, w: pw, h, origin: (x0 as f32, 0.0) },
        );
        (self.pw, self.ph) = (pw, h);
        self.dirty = false;
        let delay = out.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.repaint_at = (delay < Duration::from_secs(60)).then(|| Instant::now() + delay);
    }
}

fn rgb(c: egui::Color32) -> u32 {
    (u32::from(c.r()) << 16) | (u32::from(c.g()) << 8) | u32::from(c.b())
}
