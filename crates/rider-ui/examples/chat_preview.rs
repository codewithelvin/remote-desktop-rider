//! Draws the chat with sample messages into image files, without a window,
//! through the same CPU renderer the session window uses: for looking at
//! the layout after a change.
//!
//!   cargo run -p rider-ui --example chat_preview -- <out-dir> [dark|light]
//!
//! Writes `chat-panel.bmp` (the session window's panel, 290 pt wide),
//! `chat-page.bmp` (the app's session page width) and `chat-empty.bmp`.

use egui::{Rect, pos2, vec2};
use rider_ui::chat::Chat;
use rider_ui::soft::{SoftRenderer, Target};
use std::path::Path;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let theme = if args.next().as_deref() == Some("light") { egui::Theme::Light } else { egui::Theme::Dark };
    let dir = Path::new(&dir);

    let mut sample = Chat::new("DESKTOP-4BCTQ03");
    sample.text(false, "Hi, I can see your screen. Can you send me the error log?");
    sample.text(false, "It should be in the Logs folder.");
    sample.text(true, "Sure, one moment");
    sample.file_started(1, true, "error-2026-11-24.log", 48_213);
    sample.file_done(1);
    sample.notice("Access changed: Standard");
    sample.text(false, "Thanks! Here is the fix, run it as administrator please.");
    sample.file_started(2, false, "fix-printer-driver.zip", 3_480_000);
    sample.file_done(2);
    sample.file_started(3, false, "screenshot.png", 900_000);
    sample.file_progress(3, 420_000);
    sample.text(true, "Done, it prints now 👍");

    render(&mut sample, theme, 290.0, 560.0, 1.5, &dir.join("chat-panel.bmp"))?;
    sample.closable = false;
    sample.heading = false;
    render(&mut sample, theme, 620.0, 460.0, 1.5, &dir.join("chat-page.bmp"))?;
    render(&mut Chat::new("DESKTOP-4BCTQ03"), theme, 290.0, 400.0, 1.5, &dir.join("chat-empty.bmp"))?;
    Ok(())
}

fn render(chat: &mut Chat, theme: egui::Theme, w: f32, h: f32, ppp: f32, out: &Path) -> std::io::Result<()> {
    let ctx = egui::Context::default();
    rider_ui::theme::setup(&ctx);
    ctx.set_theme(theme);
    let mut renderer = SoftRenderer::default();
    let (pw, ph) = ((w * ppp) as usize, (h * ppp) as usize);
    let mut pixels = Vec::new();
    // A few frames: fonts load and layouts settle (scroll to the bottom).
    for _ in 0..4 {
        let raw =
            egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))), ..Default::default() };
        ctx.set_pixels_per_point(ppp);
        let out = ctx.run_ui(raw, |ui| {
            chat.show(ui);
        });
        renderer.update_textures(&out.textures_delta);
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        let bg = ctx.style_of(theme).visuals.panel_fill;
        pixels.clear();
        pixels.resize(pw * ph, (u32::from(bg.r()) << 16) | (u32::from(bg.g()) << 8) | u32::from(bg.b()));
        renderer.paint(
            &prims,
            out.pixels_per_point,
            &mut Target { buf: &mut pixels, w: pw, h: ph, origin: (0.0, 0.0) },
        );
    }
    write_bmp(out, &pixels, pw, ph)
}

/// A 32-bit top-down BMP: no image crate needed.
fn write_bmp(path: &Path, px: &[u32], w: usize, h: usize) -> std::io::Result<()> {
    let size = 54 + px.len() * 4;
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(-(h as i32)).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    for p in px {
        b.extend_from_slice(&(p | 0xFF00_0000).to_le_bytes());
    }
    std::fs::write(path, b)
}
