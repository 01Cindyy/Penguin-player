//! Helpers shared by the plugin host integration tests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mp_core::{AudioFormat, PlayerCommand, Track, TrackId};
use mp_plugins::{
    Activity, HostConfig, HostEvent, HttpClient, HttpRequest, HttpResponse, PluginHost, Presence,
    Services,
};

/// Generous, so slow machines don't produce false failures.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// A plugins folder and a data folder inside one temporary directory.
pub struct TestDirs {
    root: tempfile::TempDir,
}

impl TestDirs {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("temporary directory"),
        }
    }

    pub fn plugins(&self) -> PathBuf {
        self.root.path().join("plugins")
    }

    pub fn data(&self) -> PathBuf {
        self.root.path().join("plugin-data")
    }

    /// Writes a plugin with a default manifest and `main.lua`.
    pub fn plugin(&self, id: &str, main_lua: &str) -> PathBuf {
        self.plugin_with_manifest(id, &format!(r#"{{ "name": "{id}" }}"#), main_lua)
    }

    pub fn plugin_with_manifest(&self, id: &str, manifest: &str, main_lua: &str) -> PathBuf {
        let dir = self.plugins().join(id);
        write(&dir.join("plugin.json"), manifest);
        write(&dir.join("main.lua"), main_lua);
        dir
    }

    pub fn start(&self, enabled: &[&str]) -> (PluginHost, Events) {
        start(self.plugins(), self.data(), enabled)
    }
}

pub fn start(plugins_dir: PathBuf, data_dir: PathBuf, enabled: &[&str]) -> (PluginHost, Events) {
    start_with(config(plugins_dir, data_dir, enabled))
}

/// A host for `enabled` that can't go online, before any permissions or settings.
pub fn config(plugins_dir: PathBuf, data_dir: PathBuf, enabled: &[&str]) -> HostConfig {
    HostConfig {
        plugins_dir,
        data_dir,
        enabled: enabled.iter().map(|id| id.to_string()).collect(),
        permissions: HashMap::new(),
        settings: HashMap::new(),
        services: Services::offline(),
    }
}

pub fn start_with(config: HostConfig) -> (PluginHost, Events) {
    let (host, receiver) = PluginHost::start(config);
    (host, Events(receiver))
}

type Answer = dyn Fn(&HttpRequest) -> Result<HttpResponse, String> + Send + Sync;

/// A stand-in for the internet: answers with `answer` and remembers every request.
pub struct FakeWeb {
    answer: Box<Answer>,
    pub asked: Mutex<Vec<HttpRequest>>,
}

impl FakeWeb {
    pub fn new(
        answer: impl Fn(&HttpRequest) -> Result<HttpResponse, String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            answer: Box::new(answer),
            asked: Mutex::new(Vec::new()),
        })
    }

    pub fn urls(&self) -> Vec<String> {
        let asked = self.asked.lock().unwrap();
        asked.iter().map(|request| request.url.clone()).collect()
    }
}

impl HttpClient for FakeWeb {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        self.asked.lock().unwrap().push(request.clone());
        (self.answer)(request)
    }
}

pub fn ok(body: &str) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: 200,
        body: body.as_bytes().to_vec(),
    })
}

/// A stand-in for Discord that remembers what plugins show.
#[derive(Default)]
pub struct FakeDiscord {
    pub shown: Mutex<Vec<Option<Activity>>>,
}

impl FakeDiscord {
    /// What was shown last, waiting for it to arrive.
    pub fn last(&self) -> Option<Activity> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(last) = self.shown.lock().unwrap().last() {
                return last.clone();
            }
            assert!(Instant::now() < deadline, "nothing reached Discord");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Presence for FakeDiscord {
    fn set(&self, activity: Option<Activity>) {
        self.shown.lock().unwrap().push(activity);
    }
}

pub fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("file has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The app's end of the host event channel.
pub struct Events(pub async_channel::Receiver<HostEvent>);

impl Events {
    /// The next event, failing the test if none arrives in time.
    pub fn next(&self) -> HostEvent {
        self.try_next(TIMEOUT)
            .unwrap_or_else(|| panic!("no host event within {TIMEOUT:?}"))
    }

    pub fn try_next(&self, wait: Duration) -> Option<HostEvent> {
        let deadline = Instant::now() + wait;
        loop {
            if let Ok(event) = self.0.try_recv() {
                return Some(event);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Fails if anything arrives within `wait`.
    pub fn expect_none(&self, wait: Duration) {
        if let Some(event) = self.try_next(wait) {
            panic!("unexpected host event: {event:?}");
        }
    }

    pub fn command(&self) -> PlayerCommand {
        match self.next() {
            HostEvent::Command { command, .. } => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    /// The next notification as `(title, body)`.
    pub fn notification(&self) -> (String, Option<String>) {
        match self.next() {
            HostEvent::Notify { title, body, .. } => (title, body),
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    /// The body of the next notification, which must have `title`.
    pub fn notified(&self, title: &str) -> String {
        let (actual, body) = self.notification();
        assert_eq!(actual, title);
        body.unwrap_or_default()
    }

    /// The next host event that isn't [`HostEvent::LyricsSearch`].
    pub fn next_except_lyrics_search(&self) -> HostEvent {
        loop {
            match self.next() {
                HostEvent::LyricsSearch(_) => {}
                event => return event,
            }
        }
    }

    /// The next error as `(plugin, message)`.
    pub fn error(&self) -> (String, String) {
        match self.next() {
            HostEvent::PluginError { plugin, message } => (plugin, message),
            other => panic!("expected a plugin error, got {other:?}"),
        }
    }
}

pub fn track() -> Track {
    Track {
        id: TrackId(7),
        path: PathBuf::from("/music/Artist/Album/01 Song.flac"),
        title: "Song".into(),
        artist: "Artist".into(),
        album: "Album".into(),
        album_artist: "Album Artist".into(),
        genre: "Jazz".into(),
        year: Some(2024),
        track_number: Some(1),
        disc_number: None,
        duration_ms: 180_000,
        format: AudioFormat::Flac,
        sample_rate: Some(96_000),
        bit_depth: Some(24),
        channels: Some(2),
        bitrate_kbps: None,
        file_size: 50_000_000,
        modified: 0,
        added: 0,
        cover: None,
    }
}
