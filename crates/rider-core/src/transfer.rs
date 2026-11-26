use crate::clipboard::Image;
use crate::proto::{BulkHeader, FileHeader, FileResult, ImageHeader, read_msg, write_msg};
use crate::sharp::{RefreshHeader, TilePng};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNK: usize = 256 * 1024;
const BULK_PRIORITY: i32 = -2_000_000_000;

pub type Cancel = Arc<AtomicBool>;

pub async fn send(
    conn: &quinn::Connection,
    path: &Path,
    cancel: &Cancel,
    progress: impl FnMut(u64),
) -> Result<FileResult> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    send_open(conn, file, name, cancel, progress).await
}

pub async fn send_open(
    conn: &quinn::Connection,
    file: std::fs::File,
    name: String,
    cancel: &Cancel,
    mut progress: impl FnMut(u64),
) -> Result<FileResult> {
    let mut file = tokio::fs::File::from_std(file);
    let size = file.metadata().await?.len();
    let (mut send, mut recv) = conn.open_bi().await?;
    let _ = send.set_priority(BULK_PRIORITY);
    let header = FileHeader { name, size };
    if crate::tls::speaks_v6(conn) {
        write_msg(&mut send, &BulkHeader::File(header)).await?;
    } else {
        write_msg(&mut send, &header).await?;
    }
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = send.reset(0u32.into());
            bail!("cancelled");
        }
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await?;
        sent += n as u64;
        progress(sent);
    }
    send.finish()?;
    read_msg::<FileResult>(&mut recv).await?.context("the other side closed the transfer")
}

pub async fn send_image(conn: &quinn::Connection, image: &Image) -> Result<()> {
    let (mut send, _recv) = conn.open_bi().await?;
    let _ = send.set_priority(BULK_PRIORITY);
    let header = ImageHeader { width: image.width, height: image.height, size: image.png.len() as u64 };
    write_msg(&mut send, &BulkHeader::ClipboardImage(header)).await?;
    send.write_all(&image.png).await?;
    send.finish()?;
    let _ = send.stopped().await;
    Ok(())
}

pub async fn read_header(conn: &quinn::Connection, recv: &mut quinn::RecvStream) -> Result<BulkHeader> {
    if crate::tls::speaks_v6(conn) {
        read_msg::<BulkHeader>(recv).await?.context("missing stream header")
    } else {
        Ok(BulkHeader::File(read_msg::<FileHeader>(recv).await?.context("missing file header")?))
    }
}

pub async fn receive_image(mut recv: quinn::RecvStream, header: ImageHeader) -> Result<Image> {
    let size = usize::try_from(header.size).ok().filter(|&n| n <= crate::clipboard::MAX_PNG_BYTES);
    let Some(size) = size else {
        let _ = recv.stop(0u32.into());
        bail!("clipboard image too large ({} bytes)", header.size);
    };
    let png = recv.read_to_end(size).await.context("reading a clipboard image")?;
    if png.len() != size {
        bail!("clipboard image incomplete");
    }
    Ok(Image { width: header.width, height: header.height, png })
}

pub async fn send_refresh(conn: &quinn::Connection, header: RefreshHeader, tiles: &[TilePng]) -> Result<()> {
    let body = crate::proto::encode(&tiles)?;
    let (mut send, _recv) = conn.open_bi().await?;
    let _ = send.set_priority(BULK_PRIORITY);
    let header = RefreshHeader { size: body.len() as u64, ..header };
    write_msg(&mut send, &BulkHeader::SharpRefresh(header)).await?;
    send.write_all(&body).await?;
    send.finish()?;
    let _ = send.stopped().await;
    Ok(())
}

pub async fn receive_refresh(mut recv: quinn::RecvStream, header: RefreshHeader) -> Result<Vec<TilePng>> {
    let size = usize::try_from(header.size).ok().filter(|&n| n <= crate::sharp::MAX_REFRESH_BYTES);
    let Some(size) = size else {
        let _ = recv.stop(0u32.into());
        bail!("sharp refresh too large ({} bytes)", header.size);
    };
    let body = recv.read_to_end(size).await.context("reading a sharp refresh")?;
    crate::proto::decode(&body)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub name: String,
    pub size: u64,
    pub path: PathBuf,
}

pub async fn receive(
    header: FileHeader,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    announced: impl FnOnce(&str, u64),
    create: impl FnOnce(&str) -> Result<(PathBuf, std::fs::File)> + Send + 'static,
    cancel: &Cancel,
    progress: impl FnMut(u64),
) -> Result<Received> {
    let discard = |p: PathBuf| {
        let _ = std::fs::remove_file(p);
    };
    receive_with(header, send, recv, announced, create, discard, cancel, progress).await
}

#[expect(clippy::too_many_arguments, reason = "the callbacks differ for each receiving side")]
pub async fn receive_with(
    header: FileHeader,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    announced: impl FnOnce(&str, u64),
    create: impl FnOnce(&str) -> Result<(PathBuf, std::fs::File)> + Send + 'static,
    discard: impl FnOnce(PathBuf) + Send + 'static,
    cancel: &Cancel,
    mut progress: impl FnMut(u64),
) -> Result<Received> {
    let name = safe_file_name(&header.name).context("invalid file name")?;
    announced(&name, header.size);
    let (path, file) = {
        let name = name.clone();
        tokio::task::spawn_blocking(move || create(&name)).await??
    };
    let result = async {
        let mut file = tokio::fs::File::from_std(file);
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        while let Some(n) = recv.read(&mut buf).await? {
            if cancel.load(Ordering::Relaxed) {
                let _ = recv.stop(0u32.into());
                bail!("cancelled");
            }
            total += n as u64;
            if total > header.size {
                let _ = recv.stop(0u32.into());
                bail!("the sender went past the size it announced");
            }
            file.write_all(&buf[..n]).await?;
            progress(total);
        }
        file.flush().await?;
        if total != header.size {
            bail!("transfer incomplete");
        }
        anyhow::Ok(())
    }
    .await;
    let answer = match &result {
        Ok(()) => FileResult { ok: true, message: path.display().to_string() },
        Err(e) => FileResult { ok: false, message: format!("{e:#}") },
    };
    if result.is_err() {
        let p = path.clone();
        let _ = tokio::task::spawn_blocking(move || discard(p)).await;
    }
    let _ = write_msg(&mut send, &answer).await;
    let _ = send.finish();
    result.map(|()| Received { name, size: header.size, path })
}

pub async fn refuse(mut send: quinn::SendStream, mut recv: quinn::RecvStream, message: &str) -> Result<()> {
    let _ = recv.stop(0u32.into());
    write_msg(&mut send, &FileResult { ok: false, message: message.into() }).await?;
    let _ = send.finish();
    Ok(())
}

pub fn safe_file_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next()?;
    let name: String = base.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let name = name.trim_end_matches(['.', ' ']).trim_start();
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem[3..].chars().all(|c| c.is_ascii_digit()));
    (!name.is_empty() && !device && name.chars().count() <= 200).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::tls;

    async fn pair() -> (quinn::Connection, quinn::Connection, quinn::Endpoint, quinn::Endpoint) {
        let (host_id, client_id) = (Identity::ephemeral().unwrap(), Identity::ephemeral().unwrap());
        let server_cfg = tls::server_config(&host_id, Arc::new(tls::AnyClient)).unwrap();
        let server = quinn::Endpoint::server(server_cfg, "127.0.0.1:0".parse().unwrap()).unwrap();
        let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        client.set_default_client_config(tls::client_config(&client_id, Some(host_id.fingerprint())).unwrap());
        let addr = server.local_addr().unwrap();
        let (c, s) = tokio::join!(async { client.connect(addr, "rider").unwrap().await.unwrap() }, async {
            server.accept().await.unwrap().await.unwrap()
        });
        (c, s, client, server)
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rider-transfer-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn a_file_arrives_whole_with_progress() {
        let (client, host, _e1, _e2) = pair().await;
        let dir = scratch("whole");
        let src = dir.join("in.bin");
        let data: Vec<u8> = (0..700_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let cancel = Cancel::default();
        let receiver = {
            let (cancel, out, host) = (cancel.clone(), out.clone(), host.clone());
            tokio::spawn(async move {
                let (s, mut r) = host.accept_bi().await.unwrap();
                let BulkHeader::File(header) = read_header(&host, &mut r).await.unwrap() else { panic!("not a file") };
                let mut seen = Vec::new();
                let mut steps = 0;
                let got = receive(
                    header,
                    s,
                    r,
                    |name, size| seen.push((name.to_owned(), size)),
                    move |name| {
                        let p = out.join(name);
                        Ok((p.clone(), std::fs::File::create(p)?))
                    },
                    &cancel,
                    |_| steps += 1,
                )
                .await;
                (got, seen, steps)
            })
        };
        let mut last = 0;
        let answer = send(&client, &src, &cancel, |n| last = n).await.unwrap();
        let (got, seen, steps) = receiver.await.unwrap();
        let got = got.unwrap();
        assert!(answer.ok, "{}", answer.message);
        assert_eq!(last, data.len() as u64);
        assert_eq!(seen, [("in.bin".to_owned(), data.len() as u64)]);
        assert!(steps >= 1);
        assert_eq!(std::fs::read(&got.path).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_cancelled_file_leaves_nothing_behind() {
        let (client, host, _e1, _e2) = pair().await;
        let dir = scratch("cancel");
        let src = dir.join("big.bin");
        std::fs::write(&src, vec![1u8; 4 * 1024 * 1024]).unwrap();
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let receiver = {
            let (out, host) = (out.clone(), host.clone());
            tokio::spawn(async move {
                let (s, mut r) = host.accept_bi().await.unwrap();
                let BulkHeader::File(header) = read_header(&host, &mut r).await.unwrap() else { panic!("not a file") };
                let create = move |name: &str| {
                    let p = out.join(name);
                    Ok((p.clone(), std::fs::File::create(p)?))
                };
                receive(header, s, r, |_, _| {}, create, &Cancel::default(), |_| {}).await
            })
        };
        let cancel = Cancel::default();
        let c2 = cancel.clone();
        let sent = send(&client, &src, &cancel, move |_| c2.store(true, Ordering::Relaxed)).await;
        assert!(sent.is_err(), "the sender reports the cancel");
        assert!(receiver.await.unwrap().is_err(), "the receiver sees it fail");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "no partial file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_clipboard_image_arrives_tagged() {
        let (client, host, _e1, _e2) = pair().await;
        let rgba: Vec<u8> = (0..64u32 * 48 * 4).map(|i| (i % 251) as u8).collect();
        let image = crate::clipboard::encode_png(64, 48, &rgba).unwrap();
        let receiver = {
            let host = host.clone();
            tokio::spawn(async move {
                let (_s, mut r) = host.accept_bi().await.unwrap();
                match read_header(&host, &mut r).await.unwrap() {
                    BulkHeader::ClipboardImage(h) => receive_image(r, h).await,
                    other => panic!("taken for {other:?}"),
                }
            })
        };
        send_image(&client, &image).await.unwrap();
        let got = receiver.await.unwrap().unwrap();
        assert_eq!(got, image);
        assert_eq!(crate::clipboard::decode_png(&got).unwrap(), rgba);
    }

    #[tokio::test]
    async fn an_oversized_clipboard_image_is_refused() {
        let (client, host, _e1, _e2) = pair().await;
        let receiver = tokio::spawn(async move {
            let (_s, mut r) = host.accept_bi().await.unwrap();
            let BulkHeader::ClipboardImage(h) = read_header(&host, &mut r).await.unwrap() else {
                panic!("not an image")
            };
            receive_image(r, h).await
        });
        let (mut send, _recv) = client.open_bi().await.unwrap();
        let header = ImageHeader { width: 1, height: 1, size: crate::clipboard::MAX_PNG_BYTES as u64 + 1 };
        write_msg(&mut send, &BulkHeader::ClipboardImage(header)).await.unwrap();
        let err = receiver.await.unwrap().unwrap_err();
        assert!(format!("{err:#}").contains("too large"), "{err:#}");
    }

    #[test]
    fn incoming_file_names_are_tamed() {
        assert_eq!(safe_file_name("report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(safe_file_name("../../Windows/evil.dll").as_deref(), Some("evil.dll"));
        assert_eq!(safe_file_name("C:\\Users\\x\\a.txt").as_deref(), Some("a.txt"));
        assert_eq!(safe_file_name("a.txt:hidden").as_deref(), Some("a.txt_hidden"));
        assert_eq!(safe_file_name("notes.txt. . ").as_deref(), Some("notes.txt"));
        for bad in ["", ".", "..", "CON", "nul.txt", "Com1.log", "LPT9", "aux .txt", "/", "x/"] {
            assert_eq!(safe_file_name(bad), None, "{bad:?}");
        }
        assert_eq!(safe_file_name("COM10.txt").as_deref(), Some("COM10.txt"));
        assert_eq!(safe_file_name("console.txt").as_deref(), Some("console.txt"));
    }
}
