//! Machine-readable mode (`--json`) used by the desktop app, which runs
//! `rider-host` elevated as a child process: events go to stdout as one JSON
//! object per line; answers come back as lines on stdin (`y`, `n`, `quit`).
//! A one-time session is allowed with an access level (`y standard`, `y view`;
//! plain `y` is full access) and changed while it runs with `access <level>`.
//! Closing stdin counts as `quit`, so a crashed app never leaves the host
//! in pairing mode or the service paused.
//!
//! Chat (see `chat`): `chat "<text>"` sends a message, `send-file "<path>"`
//! a file, `cancel-file <id>` stops one (text and paths as JSON strings, so
//! any character fits on one line).

use rider_core::proto::Access;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, Notify, mpsc, watch};

static JSON: OnceLock<()> = OnceLock::new();
/// Answers: the access level allowed, `None` = no.
static ANSWERS: OnceLock<Mutex<mpsc::UnboundedReceiver<Option<Access>>>> = OnceLock::new();
/// The level asked for during a session (`access <level>` lines).
static ACCESS: OnceLock<watch::Sender<Option<Access>>> = OnceLock::new();

/// What the person at this computer does in the chat.
#[derive(Debug, PartialEq)]
pub enum ChatCmd {
    Text(String),
    SendFile(PathBuf),
    Cancel(u64),
    /// End the session (the chat window of a paired session, see `panel`).
    End,
    /// The answer to the company admin's request (`accept` / `decline`).
    Answer(bool),
}

/// A chat command line: `chat "<text>"`, `send-file "<path>"`, `cancel-file <id>`.
pub fn parse_chat(word: &str, arg: &str) -> Option<ChatCmd> {
    match word {
        "chat" => serde_json::from_str::<String>(arg).ok().map(ChatCmd::Text),
        "send-file" => serde_json::from_str::<String>(arg).ok().map(|p| ChatCmd::SendFile(p.into())),
        "cancel-file" => arg.parse().ok().map(ChatCmd::Cancel),
        _ => None,
    }
}

/// A line split into its first word and the (trimmed) rest.
pub fn split_line(line: &str) -> (&str, &str) {
    // Some callers (.NET) start the stream with a UTF-8 byte order mark.
    let line = line.trim().trim_start_matches('\u{feff}');
    line.split_once(' ').map_or((line, ""), |(w, a)| (w, a.trim()))
}

type ChatChannel = (mpsc::UnboundedSender<ChatCmd>, Mutex<mpsc::UnboundedReceiver<ChatCmd>>);
static CHAT: OnceLock<ChatChannel> = OnceLock::new();

fn chat_channel() -> &'static ChatChannel {
    CHAT.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Mutex::new(rx))
    })
}

/// The chat commands from the app; a session holds the lock while it runs.
pub fn chat_commands() -> &'static Mutex<mpsc::UnboundedReceiver<ChatCmd>> {
    &chat_channel().1
}

fn access_tx() -> &'static watch::Sender<Option<Access>> {
    ACCESS.get_or_init(|| watch::channel(None).0)
}

/// Changes of the access level asked for by the app (while a session runs).
pub fn access_changes() -> watch::Receiver<Option<Access>> {
    let mut rx = access_tx().subscribe();
    rx.mark_unchanged();
    rx
}

/// One line from the app.
#[derive(Debug, PartialEq)]
enum Line {
    Answer(Option<Access>),
    Access(Access),
    Chat(ChatCmd),
    Quit,
}

fn parse_line(line: &str) -> Option<Line> {
    let (word, arg) = split_line(line);
    match word {
        "y" | "yes" if arg.is_empty() => Some(Line::Answer(Some(Access::Full))),
        "y" | "yes" => Access::from_code(arg).map(|a| Line::Answer(Some(a))),
        "n" | "no" => Some(Line::Answer(None)),
        "access" => Access::from_code(arg).map(Line::Access),
        "quit" => Some(Line::Quit),
        _ => parse_chat(word, arg).map(Line::Chat),
    }
}

pub fn enable() {
    let _ = JSON.set(());
}

pub fn json() -> bool {
    JSON.get().is_some()
}

/// Print one event line (only in JSON mode).
pub fn emit(event: serde_json::Value) {
    if json() {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{event}");
        let _ = out.flush();
    }
}

/// Start reading stdin. The returned notify fires on `quit` or end of input.
pub fn start_input() -> Arc<Notify> {
    let quit = Arc::new(Notify::new());
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = ANSWERS.set(Mutex::new(rx));
    let q = quit.clone();
    std::thread::Builder::new()
        .name("ui-stdin".into())
        .spawn(move || {
            use std::io::BufRead;
            let mut bytes = Vec::new();
            let mut input = std::io::stdin().lock();
            loop {
                bytes.clear();
                match input.read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Err(e) => {
                        tracing::warn!("input from the app ended: {e}");
                        break;
                    }
                    // A line that is not UTF-8 is skipped, not taken as the
                    // end of input (which would end the session).
                    Ok(_) => match parse_line(&String::from_utf8_lossy(&bytes)) {
                        Some(Line::Answer(a)) => _ = tx.send(a),
                        Some(Line::Access(a)) => _ = access_tx().send(Some(a)),
                        Some(Line::Chat(c)) => _ = chat_channel().0.send(c),
                        Some(Line::Quit) => break,
                        None => {}
                    },
                }
            }
            q.notify_one();
        })
        .expect("spawn stdin thread");
    quit
}

/// Wait for the operator's yes/no answer (false if input closed).
pub async fn answer() -> bool {
    answer_access().await.is_some()
}

/// Wait for the operator's answer with the access level (`None`: no, or
/// input closed).
pub async fn answer_access() -> Option<Access> {
    match ANSWERS.get() {
        Some(rx) => rx.lock().await.recv().await.flatten(),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_from_the_app() {
        assert_eq!(parse_line("y\n"), Some(Line::Answer(Some(Access::Full))));
        assert_eq!(parse_line("\u{feff}yes"), Some(Line::Answer(Some(Access::Full))));
        assert_eq!(parse_line("y standard"), Some(Line::Answer(Some(Access::Standard))));
        assert_eq!(parse_line("y view\r\n"), Some(Line::Answer(Some(Access::ViewOnly))));
        assert_eq!(parse_line("n"), Some(Line::Answer(None)));
        assert_eq!(parse_line("access view"), Some(Line::Access(Access::ViewOnly)));
        assert_eq!(parse_line("access full"), Some(Line::Access(Access::Full)));
        assert_eq!(parse_line("quit"), Some(Line::Quit));
        // Unknown levels are ignored, never read as "yes".
        assert_eq!(parse_line("y everything"), None);
        assert_eq!(parse_line("access root"), None);
        assert_eq!(parse_line("maybe"), None);
        let chat = |t: &str| Some(Line::Chat(ChatCmd::Text(t.into())));
        assert_eq!(parse_line(r#"chat "hi\nthere \"x\"""#), chat("hi\nthere \"x\""));
        assert_eq!(
            parse_line(r#"send-file "C:\\Users\\me\\a b.txt""#),
            Some(Line::Chat(ChatCmd::SendFile(r"C:\Users\me\a b.txt".into())))
        );
        assert_eq!(parse_line("cancel-file 7"), Some(Line::Chat(ChatCmd::Cancel(7))));
        assert_eq!(parse_line("chat unquoted"), None);
    }
}
