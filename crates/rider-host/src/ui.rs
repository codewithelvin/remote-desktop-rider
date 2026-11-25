use rider_core::proto::Access;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, Notify, mpsc, watch};

static JSON: OnceLock<()> = OnceLock::new();
static ANSWERS: OnceLock<Mutex<mpsc::UnboundedReceiver<Option<Access>>>> = OnceLock::new();
static ACCESS: OnceLock<watch::Sender<Option<Access>>> = OnceLock::new();

#[derive(Debug, PartialEq)]
pub enum ChatCmd {
    Text(String),
    SendFile(PathBuf),
    Cancel(u64),
    End,
    Answer(bool),
}

pub fn parse_chat(word: &str, arg: &str) -> Option<ChatCmd> {
    match word {
        "chat" => serde_json::from_str::<String>(arg).ok().map(ChatCmd::Text),
        "send-file" => serde_json::from_str::<String>(arg).ok().map(|p| ChatCmd::SendFile(p.into())),
        "cancel-file" => arg.parse().ok().map(ChatCmd::Cancel),
        _ => None,
    }
}

pub fn split_line(line: &str) -> (&str, &str) {
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

pub fn chat_commands() -> &'static Mutex<mpsc::UnboundedReceiver<ChatCmd>> {
    &chat_channel().1
}

fn access_tx() -> &'static watch::Sender<Option<Access>> {
    ACCESS.get_or_init(|| watch::channel(None).0)
}

pub fn access_changes() -> watch::Receiver<Option<Access>> {
    let mut rx = access_tx().subscribe();
    rx.mark_unchanged();
    rx
}

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

pub fn emit(event: serde_json::Value) {
    if json() {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{event}");
        let _ = out.flush();
    }
}

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

pub async fn answer() -> bool {
    answer_access().await.is_some()
}

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
