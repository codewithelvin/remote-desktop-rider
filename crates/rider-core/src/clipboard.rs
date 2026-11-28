use anyhow::{Context, Result, ensure};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const MAX_TEXT: usize = 1024 * 1024;
pub const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
const MAX_PIXELS: u64 = 40_000_000;
const POLL: Duration = Duration::from_millis(300);
const IMAGE_POLL: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    Text(String),
    Image(Image),
}

pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Image> {
    ensure!(width > 0 && height > 0, "empty image");
    ensure!(u64::from(width) * u64::from(height) <= MAX_PIXELS, "image too large ({width}x{height})");
    ensure!(rgba.len() as u64 == u64::from(width) * u64::from(height) * 4, "image data does not match its size");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header()?;
        w.write_image_data(rgba)?;
        w.finish()?;
    }
    ensure!(png.len() <= MAX_PNG_BYTES, "image too large ({} bytes as PNG)", png.len());
    Ok(Image { width, height, png })
}

pub fn decode_png(image: &Image) -> Result<Vec<u8>> {
    ensure!(image.png.len() <= MAX_PNG_BYTES, "image too large");
    let mut dec = png::Decoder::new(std::io::Cursor::new(&image.png));
    dec.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
    );
    {
        let info = dec.read_header_info().context("not a PNG image")?;
        ensure!(
            info.width == image.width && info.height == image.height,
            "image size does not match ({}x{} announced, {}x{} inside)",
            image.width,
            image.height,
            info.width,
            info.height
        );
        ensure!(u64::from(info.width) * u64::from(info.height) <= MAX_PIXELS, "image too large");
    }
    let mut reader = dec.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size().context("image too large")?];
    let frame = reader.next_frame(&mut buf)?;
    buf.truncate(frame.buffer_size());
    match frame.color_type {
        png::ColorType::Rgba => Ok(buf),
        png::ColorType::GrayscaleAlpha => Ok(buf.as_chunks::<2>().0.iter().flat_map(|&[g, a]| [g, g, g, a]).collect()),
        other => anyhow::bail!("unexpected PNG colour type {other:?}"),
    }
}

fn digest(width: usize, height: usize, bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (width, height).hash(&mut h);
    bytes.hash(&mut h);
    h.finish()
}

trait Board {
    fn sequence(&self) -> Option<u32>;
    fn text(&mut self) -> Result<String, arboard::Error>;
    fn set_text(&mut self, text: &str) -> Result<(), arboard::Error>;
    fn image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error>;
    fn set_image(&mut self, image: arboard::ImageData<'static>) -> Result<(), arboard::Error>;
}

impl Board for arboard::Clipboard {
    fn sequence(&self) -> Option<u32> {
        sequence()
    }

    fn text(&mut self) -> Result<String, arboard::Error> {
        self.get_text()
    }

    fn set_text(&mut self, text: &str) -> Result<(), arboard::Error> {
        arboard::Clipboard::set_text(self, text)
    }

    fn image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error> {
        self.get_image()
    }

    fn set_image(&mut self, image: arboard::ImageData<'static>) -> Result<(), arboard::Error> {
        arboard::Clipboard::set_image(self, image)
    }
}

enum Incoming {
    Text(String),
    Image { digest: u64, data: arboard::ImageData<'static> },
}

impl Incoming {
    fn decode(clip: Clip) -> Option<Self> {
        match clip {
            Clip::Text(text) => Some(Self::Text(text)),
            Clip::Image(image) => match decode_png(&image) {
                Ok(rgba) => Some(Self::Image {
                    digest: digest(image.width as usize, image.height as usize, &rgba),
                    data: arboard::ImageData {
                        width: image.width as usize,
                        height: image.height as usize,
                        bytes: Cow::Owned(rgba),
                    },
                }),
                Err(e) => {
                    tracing::warn!("clipboard image from the other side refused: {e:#}");
                    None
                }
            },
        }
    }
}

struct Watcher<B: Board> {
    cb: B,
    text: String,
    image: Option<u64>,
    seq: Option<u32>,
    image_checked: Instant,
    incoming: Option<Incoming>,
    busy_reported: bool,
}

impl<B: Board> Watcher<B> {
    fn new(cb: B) -> Self {
        let mut w = Self {
            cb,
            text: String::new(),
            image: None,
            seq: None,
            image_checked: Instant::now(),
            incoming: None,
            busy_reported: false,
        };
        w.text = w.cb.text().unwrap_or_default();
        w.image = w.current_image().map(|(d, _)| d);
        w.seq = w.cb.sequence();
        w
    }

    fn current_image(&mut self) -> Option<(u64, arboard::ImageData<'static>)> {
        let img = self.cb.image().ok()?;
        Some((digest(img.width, img.height, &img.bytes), img))
    }

    fn receive(&mut self, clip: Clip) {
        if let Some(incoming) = Incoming::decode(clip) {
            self.incoming = Some(incoming);
        }
    }

    fn deliver(&mut self) {
        let Some(incoming) = self.incoming.take() else { return };
        let written = match &incoming {
            Incoming::Text(text) if *text == self.text => Ok(()),
            Incoming::Text(text) => self.cb.set_text(text).map(|()| self.text.clone_from(text)),
            Incoming::Image { digest, data } => self.cb.set_image(data.clone()).map(|()| {
                self.image = self.current_image().map(|(d, _)| d).or(Some(*digest));
                self.text.clear();
            }),
        };
        match written {
            Ok(()) => {
                self.busy_reported = false;
                self.seq = self.cb.sequence();
            }
            Err(e) => {
                if !self.busy_reported {
                    tracing::info!("clipboard busy, will retry the copy from the other side: {e}");
                    self.busy_reported = true;
                }
                self.incoming = Some(incoming);
            }
        }
    }

    fn poll(&mut self) -> Option<Clip> {
        let clip = self.local_change()?;
        self.incoming = None;
        Some(clip)
    }

    fn local_change(&mut self) -> Option<Clip> {
        let seq = self.cb.sequence();
        if seq.is_some() && seq == self.seq {
            return None;
        }
        self.seq = seq;
        if let Ok(text) = self.cb.text() {
            if text == self.text {
                return None;
            }
            self.text.clone_from(&text);
            self.image = None;
            return (text.len() <= MAX_TEXT).then_some(Clip::Text(text));
        }
        if seq.is_none() && self.image_checked.elapsed() < IMAGE_POLL {
            return None;
        }
        self.image_checked = Instant::now();
        let (d, img) = self.current_image()?;
        if Some(d) == self.image {
            return None;
        }
        self.image = Some(d);
        self.text.clear();
        match encode_png(img.width as u32, img.height as u32, &img.bytes) {
            Ok(image) => Some(Clip::Image(image)),
            Err(e) => {
                tracing::info!("clipboard image not sent: {e:#}");
                None
            }
        }
    }
}

#[cfg(windows)]
#[expect(clippy::unnecessary_wraps, reason = "None where the system has no clipboard sequence number")]
fn sequence() -> Option<u32> {
    Some(unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() })
}

#[cfg(not(windows))]
fn sequence() -> Option<u32> {
    None
}

pub fn spawn(local_changes: UnboundedSender<Clip>, remote_updates: Receiver<Clip>, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("rider-clipboard".into()).spawn(move || {
        let mut watcher = match arboard::Clipboard::new() {
            Ok(cb) => Watcher::new(cb),
            Err(e) => {
                tracing::warn!("clipboard unavailable: {e}");
                return;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            while let Ok(clip) = remote_updates.try_recv() {
                watcher.receive(clip);
            }
            watcher.deliver();
            if let Some(clip) = watcher.poll()
                && local_changes.send(clip).is_err()
            {
                return;
            }
            std::thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeBoard {
        text: Option<String>,
        image: Option<arboard::ImageData<'static>>,
        busy_writes: u32,
        seq: u32,
    }

    impl FakeBoard {
        fn write(&mut self) -> Result<(), arboard::Error> {
            if self.busy_writes > 0 {
                self.busy_writes -= 1;
                return Err(arboard::Error::ClipboardOccupied);
            }
            self.seq += 1;
            Ok(())
        }
    }

    impl Board for FakeBoard {
        fn sequence(&self) -> Option<u32> {
            Some(self.seq)
        }

        fn text(&mut self) -> Result<String, arboard::Error> {
            self.text.clone().ok_or(arboard::Error::ContentNotAvailable)
        }

        fn set_text(&mut self, text: &str) -> Result<(), arboard::Error> {
            self.write()?;
            self.text = Some(text.to_owned());
            self.image = None;
            Ok(())
        }

        fn image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error> {
            self.image.clone().ok_or(arboard::Error::ContentNotAvailable)
        }

        fn set_image(&mut self, image: arboard::ImageData<'static>) -> Result<(), arboard::Error> {
            self.write()?;
            self.image = Some(image);
            self.text = None;
            Ok(())
        }
    }

    fn remote_image() -> Clip {
        Clip::Image(encode_png(5, 4, &pixels(5, 4)).unwrap())
    }

    #[test]
    fn a_copy_from_the_other_side_waits_out_a_busy_clipboard() {
        let mut w = Watcher::new(FakeBoard { busy_writes: 3, ..FakeBoard::default() });
        w.receive(remote_image());
        for _ in 0..4 {
            w.deliver();
            assert_eq!(w.poll(), None);
        }
        assert_eq!(w.cb.image.as_ref().map(|i| i.bytes.to_vec()), Some(pixels(5, 4)));
        assert!(w.incoming.is_none());
    }

    #[test]
    fn a_newer_local_copy_wins_over_a_pending_one() {
        let mut w = Watcher::new(FakeBoard { busy_writes: u32::MAX, ..FakeBoard::default() });
        w.receive(remote_image());
        w.deliver();
        w.cb.text = Some("typed here".into());
        w.cb.seq += 1;
        assert_eq!(w.poll(), Some(Clip::Text("typed here".into())));
        assert!(w.incoming.is_none());
    }

    #[test]
    fn only_the_latest_copy_from_the_other_side_is_written() {
        let mut w = Watcher::new(FakeBoard::default());
        w.receive(remote_image());
        w.receive(Clip::Text("later".into()));
        w.deliver();
        assert_eq!(w.cb.text.as_deref(), Some("later"));
        assert_eq!(w.poll(), None);
    }

    fn pixels(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 13) as u8, (i % 7) as u8, 255 - (i % 3) as u8]).collect()
    }

    #[test]
    fn an_image_survives_png() {
        let rgba = pixels(37, 21);
        let image = encode_png(37, 21, &rgba).unwrap();
        assert!(image.png.len() < rgba.len() + 1024);
        assert_eq!(decode_png(&image).unwrap(), rgba);
    }

    #[test]
    fn a_wrong_size_is_refused() {
        let image = encode_png(8, 8, &pixels(8, 8)).unwrap();
        let lying = Image { width: 9, ..image.clone() };
        assert!(decode_png(&lying).is_err());
        assert!(decode_png(&Image { png: b"not a png".to_vec(), ..image }).is_err());
        assert!(encode_png(8, 8, &pixels(8, 7)).is_err());
        assert!(encode_png(0, 8, &[]).is_err());
    }

    #[test]
    fn a_huge_image_is_refused_before_decoding() {
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 10_000, 10_000);
            enc.set_color(png::ColorType::Rgba);
            let _ = enc.write_header().unwrap();
        }
        let err = decode_png(&Image { width: 10_000, height: 10_000, png }).unwrap_err();
        assert!(format!("{err:#}").contains("too large"), "{err:#}");
    }

    #[test]
    fn grey_and_rgb_images_come_out_as_rgba() {
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 2, 1);
            enc.set_color(png::ColorType::Rgb);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[1, 2, 3, 4, 5, 6]).unwrap();
        }
        assert_eq!(decode_png(&Image { width: 2, height: 1, png }).unwrap(), [1, 2, 3, 255, 4, 5, 6, 255]);
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[9, 200]).unwrap();
        }
        assert_eq!(decode_png(&Image { width: 2, height: 1, png }).unwrap(), [9, 9, 9, 255, 200, 200, 200, 255]);
    }
}
