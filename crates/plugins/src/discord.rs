//! Discord Rich Presence for plugins the user allowed to use it (`mp.discord`): what is
//! playing, shown on the user's Discord profile.
//!
//! The Discord app on the same computer listens on a local socket (`discord-ipc-0`).
//! Messages are frames of an opcode, a length and JSON: a handshake naming the app's
//! Discord application, then `SET_ACTIVITY` commands. Nothing goes over the internet
//! from here, and nothing happens while Discord isn't running.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Updates are sent at most this often: Discord accepts five per 20 seconds. Later
/// ones wait, and only the newest is sent.
const MIN_INTERVAL: Duration = Duration::from_secs(4);
/// While Discord isn't running, connecting is tried again this often.
const RETRY_INTERVAL: Duration = Duration::from_secs(15);
/// Longest wait for Discord to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Discord cuts texts longer than this.
pub(crate) const MAX_TEXT_CHARS: usize = 128;

/// What kind of activity Discord says the user is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityKind {
    Playing,
    #[default]
    Listening,
    Watching,
    Competing,
}

impl ActivityKind {
    pub const ALL: [ActivityKind; 4] = [
        ActivityKind::Playing,
        ActivityKind::Listening,
        ActivityKind::Watching,
        ActivityKind::Competing,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ActivityKind::Playing => "playing",
            ActivityKind::Listening => "listening",
            ActivityKind::Watching => "watching",
            ActivityKind::Competing => "competing",
        }
    }

    fn code(self) -> u8 {
        match self {
            ActivityKind::Playing => 0,
            ActivityKind::Listening => 2,
            ActivityKind::Watching => 3,
            ActivityKind::Competing => 5,
        }
    }
}

/// Which text Discord shows after "Listening to" in member lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusDisplay {
    /// The app's name, as registered with Discord.
    #[default]
    Name,
    /// The second line ([`Activity::state`]).
    State,
    /// The first line ([`Activity::details`]).
    Details,
}

impl StatusDisplay {
    pub const ALL: [StatusDisplay; 3] = [
        StatusDisplay::Name,
        StatusDisplay::State,
        StatusDisplay::Details,
    ];

    pub fn name(self) -> &'static str {
        match self {
            StatusDisplay::Name => "name",
            StatusDisplay::State => "state",
            StatusDisplay::Details => "details",
        }
    }

    fn code(self) -> u8 {
        match self {
            StatusDisplay::Name => 0,
            StatusDisplay::State => 1,
            StatusDisplay::Details => 2,
        }
    }
}

/// What a plugin shows on Discord. Texts are at most [`MAX_TEXT_CHARS`] long.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Activity {
    pub kind: ActivityKind,
    /// The first line, such as the song's title.
    pub details: Option<String>,
    /// The second line, such as the artist.
    pub state: Option<String>,
    /// An image uploaded to the app's Discord application, by name, or an `https://` URL.
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    /// When the song started and will end, in milliseconds since 1970, for the
    /// progress bar.
    pub started_ms: Option<u64>,
    pub ends_ms: Option<u64>,
    pub status_display: StatusDisplay,
}

impl Activity {
    /// Sets the progress bar for a song `elapsed` into it, `duration` long.
    pub fn set_progress(&mut self, elapsed: Duration, duration: Option<Duration>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let started = now.saturating_sub(elapsed);
        self.started_ms = Some(millis(started));
        self.ends_ms = duration.map(|duration| millis(started + duration));
    }

    fn to_json(&self) -> Value {
        let mut activity = json!({
            "type": self.kind.code(),
            "status_display_type": self.status_display.code(),
            "instance": false,
        });
        let fields = [("details", &self.details), ("state", &self.state)];
        for (name, value) in fields {
            if let Some(value) = value {
                activity[name] = json!(value);
            }
        }
        let mut assets = serde_json::Map::new();
        let images = [
            ("large_image", &self.large_image),
            ("large_text", &self.large_text),
            ("small_image", &self.small_image),
            ("small_text", &self.small_text),
        ];
        for (name, value) in images {
            if let Some(value) = value {
                assets.insert(name.to_owned(), json!(value));
            }
        }
        if !assets.is_empty() {
            activity["assets"] = Value::Object(assets);
        }
        if let Some(start) = self.started_ms {
            activity["timestamps"] = json!({ "start": start });
            if let Some(end) = self.ends_ms {
                activity["timestamps"]["end"] = json!(end);
            }
        }
        activity
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Where activities go: [`DiscordPresence`] in the app, a recorder in tests.
pub trait Presence: Send + Sync {
    /// Shows `activity` on the user's profile, or nothing for `None`.
    fn set(&self, activity: Option<Activity>);
}

/// Shows nothing. Plugins get it where Discord isn't available.
pub struct NoPresence;

impl Presence for NoPresence {
    fn set(&self, _activity: Option<Activity>) {}
}

/// How [`DiscordPresence`] reaches Discord.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// The id of the app's application in Discord's developer portal. Its name is
    /// what Discord shows ("Listening to …"), and its images can be shown by name.
    pub application_id: String,
    /// Where Discord's socket may be, tried in order.
    pub sockets: Vec<std::path::PathBuf>,
    pub min_interval: Duration,
    pub retry_interval: Duration,
}

impl DiscordConfig {
    pub fn new(application_id: &str) -> Self {
        Self {
            application_id: application_id.to_owned(),
            sockets: socket_paths(),
            min_interval: MIN_INTERVAL,
            retry_interval: RETRY_INTERVAL,
        }
    }
}

/// Talks to the Discord app on a thread of its own, started when a plugin first shows
/// something. Dropping it clears the activity.
pub struct DiscordPresence {
    config: DiscordConfig,
    updates: Mutex<Option<mpsc::Sender<Option<Activity>>>>,
    /// Set once the log says that Discord isn't set up, so it says so only once.
    told_unset: AtomicBool,
}

impl DiscordPresence {
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            updates: Mutex::new(None),
            told_unset: AtomicBool::new(false),
        }
    }

    fn sender(&self) -> Option<mpsc::Sender<Option<Activity>>> {
        let mut updates = self.updates.lock().unwrap_or_else(|err| err.into_inner());
        if updates.is_none() {
            if self.config.application_id.is_empty() {
                if !self.told_unset.swap(true, Ordering::Relaxed) {
                    log::warn!("Discord isn't set up in this build: it has no application id");
                }
                return None;
            }
            let (sender, receiver) = mpsc::channel();
            let config = self.config.clone();
            let spawned = std::thread::Builder::new()
                .name("discord".to_owned())
                .spawn(move || run(&config, &receiver));
            if let Err(err) = spawned {
                log::error!("Could not start the Discord thread: {err}");
                return None;
            }
            *updates = Some(sender);
        }
        updates.clone()
    }
}

impl Presence for DiscordPresence {
    fn set(&self, activity: Option<Activity>) {
        let started = self
            .updates
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_some();
        // Nothing to clear before anything was shown.
        if activity.is_none() && !started {
            return;
        }
        if let Some(sender) = self.sender() {
            let _ = sender.send(activity);
        }
    }
}

/// The Discord thread: sends the newest activity whenever allowed, connecting (and
/// reconnecting after Discord restarts) as needed. Ends, clearing the activity, when
/// the [`DiscordPresence`] is dropped.
fn run(config: &DiscordConfig, updates: &mpsc::Receiver<Option<Activity>>) {
    let mut connection: Option<Connection> = None;
    let mut wanted: Option<Activity> = None;
    let mut unsent = false;
    let mut next_send = Instant::now();
    loop {
        let update = if unsent {
            updates.recv_timeout(next_send.saturating_duration_since(Instant::now()))
        } else {
            updates.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match update {
            Ok(activity) => {
                wanted = activity;
                unsent = true;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() < next_send {
            continue;
        }
        if wanted.is_none() && connection.is_none() {
            unsent = false;
            continue;
        }
        if connection.is_none() {
            connection = Connection::open(config);
            if connection.is_none() {
                next_send = Instant::now() + config.retry_interval;
                continue;
            }
        }
        let Some(open) = connection.as_mut() else {
            continue;
        };
        match open.set_activity(wanted.as_ref()) {
            Ok(()) => {
                unsent = false;
                next_send = Instant::now() + config.min_interval;
            }
            Err(SendError::Refused(message)) => {
                log::warn!("Discord refused the activity: {message}");
                unsent = false;
                next_send = Instant::now() + config.min_interval;
            }
            Err(SendError::Disconnected(err)) => {
                log::info!("Lost the connection to Discord: {err}");
                connection = None;
                next_send = Instant::now() + config.retry_interval;
            }
        }
    }
    if let Some(mut connection) = connection {
        let _ = connection.set_activity(None);
    }
}

enum SendError {
    /// Discord answered with an error, such as for a text it doesn't accept.
    Refused(String),
    Disconnected(std::io::Error),
}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
/// Discord's answers are small; anything larger is not Discord.
const MAX_FRAME_BYTES: u32 = 64 * 1024;

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

#[cfg(unix)]
fn connect(path: &std::path::Path) -> std::io::Result<Stream> {
    let stream = Stream::connect(path)?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    Ok(stream)
}

// Discord on Windows listens on the named pipe `\\.\pipe\discord-ipc-0` instead.
#[cfg(not(unix))]
type Stream = std::fs::File;

#[cfg(not(unix))]
fn connect(_path: &std::path::Path) -> std::io::Result<Stream> {
    Err(std::io::ErrorKind::Unsupported.into())
}

struct Connection {
    stream: Stream,
    nonce: u64,
}

impl Connection {
    /// Connects to the first Discord socket that answers the handshake.
    fn open(config: &DiscordConfig) -> Option<Self> {
        for path in &config.sockets {
            let Ok(stream) = connect(path) else {
                continue;
            };
            let mut connection = Self { stream, nonce: 0 };
            match connection.handshake(&config.application_id) {
                Ok(()) => {
                    log::info!("Connected to Discord at {}", path.display());
                    return Some(connection);
                }
                Err(err) => log::warn!("Discord at {} refused us: {err}", path.display()),
            }
        }
        None
    }

    fn handshake(&mut self, application_id: &str) -> std::io::Result<()> {
        self.write(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": application_id }),
        )?;
        let (op, reply) = self.read()?;
        if op == OP_FRAME && reply["evt"] == "READY" {
            Ok(())
        } else {
            Err(std::io::Error::other(error_message(&reply)))
        }
    }

    fn set_activity(&mut self, activity: Option<&Activity>) -> Result<(), SendError> {
        self.nonce += 1;
        let shown = activity.map(Activity::to_json);
        log::debug!(
            "Showing on Discord: {}",
            shown.as_ref().unwrap_or(&Value::Null)
        );
        let command = json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": shown,
            },
            "nonce": self.nonce.to_string(),
        });
        self.write(OP_FRAME, &command)
            .map_err(SendError::Disconnected)?;
        let (op, reply) = self.read().map_err(SendError::Disconnected)?;
        if op == OP_CLOSE {
            return Err(SendError::Disconnected(std::io::Error::other(
                error_message(&reply),
            )));
        }
        if reply["evt"] == "ERROR" {
            return Err(SendError::Refused(error_message(&reply)));
        }
        Ok(())
    }

    fn write(&mut self, op: u32, message: &Value) -> std::io::Result<()> {
        use std::io::Write;
        let body = message.to_string();
        let length = u32::try_from(body.len()).map_err(std::io::Error::other)?;
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.stream.write_all(&frame)
    }

    fn read(&mut self) -> std::io::Result<(u32, Value)> {
        use std::io::Read;
        let mut header = [0; 8];
        self.stream.read_exact(&mut header)?;
        let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if length > MAX_FRAME_BYTES {
            return Err(std::io::Error::other("Discord sent an oversized message"));
        }
        let mut body = vec![0; length as usize];
        self.stream.read_exact(&mut body)?;
        let message = serde_json::from_slice(&body).map_err(std::io::Error::other)?;
        Ok((op, message))
    }
}

/// The message in one of Discord's error replies.
fn error_message(reply: &Value) -> String {
    reply["data"]["message"]
        .as_str()
        .or_else(|| reply["message"].as_str())
        .unwrap_or("no reason given")
        .to_owned()
}

/// Where the Discord app (or Vesktop, or Discord from Flatpak or Snap) may listen.
fn socket_paths() -> Vec<std::path::PathBuf> {
    const SUBFOLDERS: [&str; 6] = [
        "",
        "app/com.discordapp.Discord",
        "app/com.discordapp.DiscordCanary",
        ".flatpak/dev.vencord.Vesktop/xdg-run",
        "snap.discord",
        "snap.discord-canary",
    ];
    let mut folders: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    folders.push("/tmp".into());
    folders.dedup();
    let mut paths = Vec::new();
    for folder in &folders {
        for subfolder in SUBFOLDERS {
            for number in 0..10 {
                paths.push(folder.join(subfolder).join(format!("discord-ipc-{number}")));
            }
        }
    }
    paths
}

/// Sends what plugins show to a [`Presence`], remembering which plugin showed it so
/// it can be cleared when that plugin stops.
pub(crate) struct SharedPresence {
    presence: Arc<dyn Presence>,
    owner: RefCell<Option<String>>,
}

impl SharedPresence {
    pub fn new(presence: Arc<dyn Presence>) -> Self {
        Self {
            presence,
            owner: RefCell::new(None),
        }
    }

    pub fn set(&self, plugin: &str, activity: Option<Activity>) {
        let mut owner = self.owner.borrow_mut();
        match activity {
            Some(activity) => {
                *owner = Some(plugin.to_owned());
                self.presence.set(Some(activity));
            }
            None if owner.as_deref() == Some(plugin) => {
                *owner = None;
                self.presence.set(None);
            }
            None => {}
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::*;

    /// A stand-in for the Discord app: answers the handshake and every command, and
    /// passes on what it receives.
    fn fake_discord(path: &std::path::Path) -> mpsc::Receiver<Value> {
        let listener = UnixListener::bind(path).unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                while let Some((op, message)) = read_frame(&mut stream) {
                    let reply = if op == OP_HANDSHAKE {
                        json!({ "cmd": "DISPATCH", "evt": "READY" })
                    } else if message["args"]["activity"]["details"] == "bad" {
                        json!({ "evt": "ERROR", "data": { "message": "bad details" } })
                    } else {
                        json!({ "cmd": "SET_ACTIVITY", "evt": null })
                    };
                    write_frame(&mut stream, OP_FRAME, &reply);
                    if sender.send(message).is_err() {
                        return;
                    }
                }
            }
        });
        receiver
    }

    fn read_frame(stream: &mut UnixStream) -> Option<(u32, Value)> {
        let mut header = [0; 8];
        stream.read_exact(&mut header).ok()?;
        let op = u32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..].try_into().unwrap());
        let mut body = vec![0; length as usize];
        stream.read_exact(&mut body).ok()?;
        Some((op, serde_json::from_slice(&body).unwrap()))
    }

    fn write_frame(stream: &mut UnixStream, op: u32, message: &Value) {
        let body = message.to_string();
        let mut frame = op.to_le_bytes().to_vec();
        frame.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        stream.write_all(&frame).unwrap();
    }

    fn config(socket: &std::path::Path) -> DiscordConfig {
        DiscordConfig {
            application_id: "1234".to_owned(),
            sockets: vec![socket.with_extension("missing"), socket.to_path_buf()],
            min_interval: Duration::from_millis(100),
            retry_interval: Duration::from_millis(50),
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn activities_reach_discord() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("discord-ipc-0");
        let received = fake_discord(&socket);
        let presence = DiscordPresence::new(config(&socket));

        let mut activity = Activity {
            details: Some("Song".into()),
            state: Some("Artist".into()),
            large_image: Some("penguin".into()),
            ..Activity::default()
        };
        activity.set_progress(Duration::from_secs(10), Some(Duration::from_secs(60)));
        presence.set(Some(activity));

        let handshake = received.recv_timeout(WAIT).unwrap();
        assert_eq!(handshake, json!({ "v": 1, "client_id": "1234" }));
        let command = received.recv_timeout(WAIT).unwrap();
        assert_eq!(command["cmd"], "SET_ACTIVITY");
        assert_eq!(command["args"]["pid"], std::process::id());
        let sent = &command["args"]["activity"];
        assert_eq!(sent["type"], 2);
        assert_eq!(sent["details"], "Song");
        assert_eq!(sent["state"], "Artist");
        assert_eq!(sent["assets"], json!({ "large_image": "penguin" }));
        let start = sent["timestamps"]["start"].as_u64().unwrap();
        assert_eq!(sent["timestamps"]["end"].as_u64().unwrap() - start, 60_000);

        presence.set(None);
        let cleared = received.recv_timeout(WAIT).unwrap();
        assert_eq!(cleared["args"]["activity"], Value::Null);
    }

    #[test]
    fn only_the_newest_activity_is_sent_when_updates_come_fast() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("discord-ipc-0");
        let received = fake_discord(&socket);
        let presence = DiscordPresence::new(config(&socket));
        let titled = |title: &str| Activity {
            details: Some(title.into()),
            ..Activity::default()
        };

        presence.set(Some(titled("one")));
        received.recv_timeout(WAIT).unwrap();
        assert_eq!(
            received.recv_timeout(WAIT).unwrap()["args"]["activity"]["details"],
            "one"
        );
        for title in ["two", "three", "four"] {
            presence.set(Some(titled(title)));
        }
        assert_eq!(
            received.recv_timeout(WAIT).unwrap()["args"]["activity"]["details"],
            "four"
        );
        assert!(received.recv_timeout(Duration::from_millis(300)).is_err());

        // An activity Discord refuses isn't sent again and again.
        presence.set(Some(titled("bad")));
        received.recv_timeout(WAIT).unwrap();
        assert!(received.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn connects_once_discord_starts() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("discord-ipc-0");
        let presence = DiscordPresence::new(config(&socket));
        presence.set(Some(Activity::default()));
        std::thread::sleep(Duration::from_millis(120));

        let received = fake_discord(&socket);
        received.recv_timeout(WAIT).unwrap();
        assert_eq!(received.recv_timeout(WAIT).unwrap()["cmd"], "SET_ACTIVITY");
    }

    #[test]
    fn nothing_starts_without_an_application_id() {
        let presence = DiscordPresence::new(DiscordConfig::new(""));
        presence.set(Some(Activity::default()));
        assert!(presence.updates.lock().unwrap().is_none());
    }

    #[test]
    fn a_plugin_clears_only_what_it_showed() {
        struct Recorder(Mutex<Vec<Option<Activity>>>);
        impl Presence for Recorder {
            fn set(&self, activity: Option<Activity>) {
                self.0.lock().unwrap().push(activity);
            }
        }
        let recorder = Arc::new(Recorder(Mutex::new(Vec::new())));
        let shared = SharedPresence::new(recorder.clone());
        shared.set("a", Some(Activity::default()));
        shared.set("b", None);
        shared.set("b", Some(Activity::default()));
        shared.set("a", None);
        shared.set("b", None);
        let calls: Vec<bool> = recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .map(Option::is_some)
            .collect();
        assert_eq!(calls, [true, true, false]);
    }
}
