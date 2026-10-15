//! What plugins reach beyond the sandbox: web requests, Discord, lyrics and settings,
//! through the real host with stand-ins for the internet and Discord. Includes the
//! plugins shipped with the app.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{FakeDiscord, FakeWeb, TestDirs, config, ok, start_with, track};
use mp_core::{PlaybackState, PlayerEvent, PluginPermissions};
use mp_plugins::{
    ActivityKind, HostEvent, HttpRequest, HttpResponse, LyricsSearch, OwnLyrics, StatusDisplay,
};
use serde_json::json;

fn network(hosts: &[&str]) -> PluginPermissions {
    PluginPermissions {
        network: hosts.iter().map(|host| host.to_string()).collect(),
        discord: false,
    }
}

const ASKS_FOR_EXAMPLE_COM: &str =
    r#"{ "name": "Web", "permissions": { "network": ["example.com"] } }"#;

#[test]
fn web_requests_go_only_to_allowed_sites() {
    let dirs = TestDirs::new();
    dirs.plugin_with_manifest(
        "web",
        ASKS_FOR_EXAMPLE_COM,
        r#"
        mp.http.get("https://example.com/find", { query = { q = "a b", page = 2 } },
          function(response, err)
            mp.notify(tostring(response.status), response.body)
          end)
        mp.http.get("https://example.com/down", function(response, err)
          mp.notify("failed", err)
        end)
        local ok, problem = pcall(mp.http.get, "https://elsewhere.org/", function() end)
        mp.notify("elsewhere", problem)
        "#,
    );
    let web = FakeWeb::new(|request| {
        if request.url.contains("down") {
            Err("no route to host".into())
        } else {
            Ok(HttpResponse {
                status: 404,
                body: b"nothing here".to_vec(),
            })
        }
    });
    let mut config = config(dirs.plugins(), dirs.data(), &["web"]);
    config
        .permissions
        .insert("web".into(), network(&["example.com"]));
    config.services.http = web.clone();
    let (_host, events) = start_with(config);

    let body = events.notified("elsewhere");
    assert!(body.contains("may not connect to elsewhere.org"), "{body}");
    let mut answers = [events.notification(), events.notification()];
    answers.sort();
    assert_eq!(
        answers,
        [
            ("404".to_owned(), Some("nothing here".to_owned())),
            ("failed".to_owned(), Some("no route to host".to_owned())),
        ]
    );
    let mut urls = web.urls();
    urls.sort();
    assert_eq!(
        urls,
        [
            "https://example.com/down",
            "https://example.com/find?page=2&q=a%20b"
        ]
    );
}

#[test]
fn the_internet_needs_the_users_permission() {
    let dirs = TestDirs::new();
    let code = r#"
        local ok, problem = pcall(mp.http.get, "https://example.com/", function() end)
        mp.notify("refused", problem)
        "#;
    dirs.plugin_with_manifest("asks", ASKS_FOR_EXAMPLE_COM, code);
    dirs.plugin("doesnt-ask", code);
    let web = FakeWeb::new(|_| ok(""));
    let mut config = config(dirs.plugins(), dirs.data(), &["asks", "doesnt-ask"]);
    // Allowed sites a plugin doesn't ask for don't count either.
    config
        .permissions
        .insert("doesnt-ask".into(), network(&["example.com"]));
    config.services.http = web.clone();
    let (_host, events) = start_with(config);

    for _ in 0..2 {
        assert!(
            events
                .notified("refused")
                .contains("may not use the internet")
        );
    }
    assert!(web.urls().is_empty());
}

#[test]
fn settings_have_defaults_and_report_changes() {
    let dirs = TestDirs::new();
    dirs.plugin_with_manifest(
        "tuned",
        r#"{ "name": "Tuned", "settings": [
            { "key": "loud", "type": "boolean", "title": "Loud" },
            { "key": "minutes", "type": "number", "title": "Minutes", "default": 30, "min": 1, "max": 90 }
        ] }"#,
        r#"
        mp.notify("start", tostring(mp.settings.get("loud")) .. " " .. mp.settings.get("minutes"))
        mp.on("setting_changed", function(key, value)
          mp.notify(key, tostring(value) .. " " .. tostring(mp.settings.get(key)))
        end)
        local ok, problem = pcall(mp.settings.get, "nope")
        mp.notify("unknown", problem)
        "#,
    );
    let mut config = config(dirs.plugins(), dirs.data(), &["tuned"]);
    config.settings.insert(
        "tuned".into(),
        serde_json::from_value(json!({ "loud": true })).unwrap(),
    );
    let (host, events) = start_with(config);

    assert_eq!(events.notified("start"), "true 30");
    assert!(
        events
            .notified("unknown")
            .contains("no setting called 'nope'")
    );
    host.set_setting("tuned", "minutes", Some(json!(45)));
    assert_eq!(events.notified("minutes"), "45 45");
    host.set_setting("tuned", "loud", None);
    assert_eq!(events.notified("loud"), "false false");
    host.set_setting("tuned", "missing", Some(json!(1)));
    events.expect_none(Duration::from_millis(100));
}

#[test]
fn lyrics_come_from_plugins_that_find_them() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "finder",
        r#"
        mp.lyrics.provide(function(track, done)
          if track.title == "Song" then
            mp.timer.after(10, function() done("[00:01.00]" .. track.artist) end)
          elseif track.title == "Broken" then
            error("broken finder")
          else
            done(nil)
          end
        end)
        "#,
    );
    let (host, events) = dirs.start(&["finder"]);
    assert_eq!(
        events.next(),
        HostEvent::LyricsSearch(LyricsSearch::Missing)
    );

    host.find_lyrics(1, track(), OwnLyrics::Missing);
    assert_eq!(
        events.next(),
        HostEvent::Lyrics {
            request: 1,
            lyrics: Some("[00:01.00]Artist".into())
        }
    );
    let mut other = track();
    other.title = "Other".into();
    host.find_lyrics(2, other, OwnLyrics::Missing);
    assert_eq!(
        events.next(),
        HostEvent::Lyrics {
            request: 2,
            lyrics: None
        }
    );
    let mut broken = track();
    broken.title = "Broken".into();
    host.find_lyrics(3, broken, OwnLyrics::Missing);
    let (_, message) = events.error();
    assert!(message.contains("broken finder"), "{message}");
    assert_eq!(
        events.next(),
        HostEvent::Lyrics {
            request: 3,
            lyrics: None
        }
    );

    host.set_enabled("finder", false);
    assert_eq!(events.next(), HostEvent::LyricsSearch(LyricsSearch::Off));
    host.find_lyrics(4, track(), OwnLyrics::Missing);
    assert_eq!(
        events.next(),
        HostEvent::Lyrics {
            request: 4,
            lyrics: None
        }
    );
}

#[test]
fn lyrics_finders_replace_only_the_lyrics_they_say() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "finder",
        r#"
        local ok, problem = pcall(mp.lyrics.provide, function() end, { replace = "some" })
        if not ok then mp.notify("refused", problem) end
        mp.lyrics.provide(function(track, done)
          done("[00:01.00] " .. tostring(track.lyrics))
        end, { replace = "untimed" })
        "#,
    );
    let (host, events) = dirs.start(&["finder"]);
    assert!(
        events
            .notified("refused")
            .contains("'replace' must be one of 'none', 'untimed', 'all', not 'some'")
    );
    assert_eq!(
        events.next(),
        HostEvent::LyricsSearch(LyricsSearch::Untimed)
    );

    for (request, own, found) in [
        (1, OwnLyrics::Missing, Some("[00:01.00] nil")),
        (2, OwnLyrics::Untimed, Some("[00:01.00] untimed")),
        (3, OwnLyrics::Timed, None),
    ] {
        host.find_lyrics(request, track(), own);
        assert_eq!(
            events.next(),
            HostEvent::Lyrics {
                request,
                lyrics: found.map(str::to_owned)
            }
        );
    }
}

#[test]
fn discord_needs_the_users_permission() {
    let dirs = TestDirs::new();
    let manifest = r#"{ "name": "Status", "permissions": { "discord": true } }"#;
    let code = r#"
        mp.on("state_changed", function(state)
          local ok, problem = pcall(mp.discord.set_activity, {
            details = "Song", state = "Artist", large_image = "penguin",
            elapsed_ms = 1000, duration_ms = 5000, status_display = "state",
          })
          if not ok then mp.notify("refused", problem) end
        end)
        "#;
    dirs.plugin_with_manifest("allowed", manifest, code);
    dirs.plugin_with_manifest("not-allowed", manifest, code);
    let discord = Arc::new(FakeDiscord::default());
    let mut config = config(dirs.plugins(), dirs.data(), &["allowed", "not-allowed"]);
    config.permissions.insert(
        "allowed".into(),
        PluginPermissions {
            network: Vec::new(),
            discord: true,
        },
    );
    config.services.presence = discord.clone();
    let (host, events) = start_with(config);

    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Playing));
    assert!(events.notified("refused").contains("may not use Discord"));
    let shown = discord.last().unwrap();
    assert_eq!(shown.kind, ActivityKind::Listening);
    assert_eq!(shown.details.as_deref(), Some("Song"));
    assert_eq!(shown.state.as_deref(), Some("Artist"));
    assert_eq!(shown.large_image.as_deref(), Some("penguin"));
    assert_eq!(shown.status_display, StatusDisplay::State);
    assert_eq!(shown.ends_ms.unwrap() - shown.started_ms.unwrap(), 5000);

    // What a plugin showed goes away with it.
    host.set_enabled("allowed", false);
    events.expect_none(Duration::from_millis(100));
    assert_eq!(discord.shown.lock().unwrap().last(), Some(&None));
}

const LRCLIB_ANSWER: &str = r#"[
    { "trackName": "Song", "artistName": "Artist", "albumName": "Other Album",
      "duration": 180.5, "instrumental": false,
      "plainLyrics": "wrong album", "syncedLyrics": "[00:01.00] wrong album" },
    { "trackName": "Song", "artistName": "Artist", "albumName": "Album",
      "duration": 179.0, "instrumental": false,
      "plainLyrics": "right", "syncedLyrics": "[00:01.00] right" },
    { "trackName": "Song (Live)", "artistName": "Artist", "albumName": "Album",
      "duration": 260.0, "instrumental": false,
      "plainLyrics": "too long", "syncedLyrics": "[00:01.00] too long" }
]"#;

fn bundled_host(
    web: &Arc<FakeWeb>,
    discord: &Arc<FakeDiscord>,
    dirs: &TestDirs,
) -> (mp_plugins::PluginHost, common::Events) {
    let mut config = config(
        dirs.plugins(),
        dirs.data(),
        &["online-lyrics", "discord-status"],
    );
    config
        .permissions
        .insert("online-lyrics".into(), network(&["lrclib.net"]));
    config.permissions.insert(
        "discord-status".into(),
        PluginPermissions {
            network: Vec::new(),
            discord: true,
        },
    );
    config.services.http = web.clone();
    config.services.presence = discord.clone();
    start_with(config)
}

#[test]
fn online_lyrics_picks_the_best_match_and_sends_only_title_and_artist() {
    let dirs = TestDirs::new();
    let web = FakeWeb::new(|_| ok(LRCLIB_ANSWER));
    let discord = Arc::new(FakeDiscord::default());
    let (host, events) = bundled_host(&web, &discord, &dirs);
    assert_eq!(
        events.next(),
        HostEvent::LyricsSearch(LyricsSearch::Missing)
    );

    host.find_lyrics(1, track(), OwnLyrics::Missing);
    assert_eq!(
        events.next_except_lyrics_search(),
        HostEvent::Lyrics {
            request: 1,
            lyrics: Some("[00:01.00] right".into())
        }
    );
    assert_eq!(
        web.urls(),
        ["https://lrclib.net/api/search?artist_name=Artist&track_name=Song"]
    );
}

#[test]
fn online_lyrics_tries_a_simpler_title_and_remembers_songs_without_lyrics() {
    let dirs = TestDirs::new();
    let web = FakeWeb::new(|_| ok("[]"));
    let discord = Arc::new(FakeDiscord::default());
    let (host, events) = bundled_host(&web, &discord, &dirs);
    let mut remaster = track();
    remaster.title = "Song (2011 Remaster)".into();

    host.find_lyrics(1, remaster.clone(), OwnLyrics::Missing);
    assert_eq!(
        events.next_except_lyrics_search(),
        HostEvent::Lyrics {
            request: 1,
            lyrics: None
        }
    );
    assert_eq!(
        web.urls(),
        [
            "https://lrclib.net/api/search?artist_name=Artist&track_name=Song%20%282011%20Remaster%29",
            "https://lrclib.net/api/search?artist_name=Artist&track_name=Song",
        ]
    );
    host.find_lyrics(2, remaster, OwnLyrics::Missing);
    assert_eq!(
        events.next_except_lyrics_search(),
        HostEvent::Lyrics {
            request: 2,
            lyrics: None
        }
    );
    assert_eq!(web.urls().len(), 2, "asked again for a song without lyrics");
}

#[test]
fn online_lyrics_replaces_songs_own_lyrics_only_with_better_ones() {
    let dirs = TestDirs::new();
    // LRCLIB has untimed lyrics for "Song" and timed ones for "Other".
    let web = FakeWeb::new(|request: &HttpRequest| {
        let (title, lyrics) = if request.url.contains("track_name=Song") {
            (
                "Song",
                r#""plainLyrics": "plain only", "syncedLyrics": null"#,
            )
        } else {
            (
                "Other",
                r#""plainLyrics": "other", "syncedLyrics": "[00:01.00] other""#,
            )
        };
        ok(&format!(
            r#"[{{ "trackName": "{title}", "artistName": "Artist", "albumName": "Album",
                  "duration": 180.0, "instrumental": false, {lyrics} }}]"#
        ))
    });
    let discord = Arc::new(FakeDiscord::default());
    let (host, events) = bundled_host(&web, &discord, &dirs);
    let mut other = track();
    other.title = "Other".into();
    let find = |request, track: &mp_core::Track, own| {
        host.find_lyrics(request, track.clone(), own);
        match events.next_except_lyrics_search() {
            HostEvent::Lyrics { lyrics, .. } => lyrics,
            event => panic!("unexpected {event:?}"),
        }
    };

    host.set_setting("online-lyrics", "prefer", Some(json!("untimed")));
    assert_eq!(
        find(1, &track(), OwnLyrics::Untimed),
        None,
        "only untimed online"
    );
    assert_eq!(web.urls().len(), 1);
    assert_eq!(find(2, &track(), OwnLyrics::Untimed), None);
    assert_eq!(
        web.urls().len(),
        1,
        "asked again for timed lyrics LRCLIB lacks"
    );
    assert_eq!(
        find(3, &track(), OwnLyrics::Missing).as_deref(),
        Some("plain only")
    );
    assert_eq!(web.urls().len(), 2);
    assert_eq!(find(4, &other, OwnLyrics::Timed), None);
    assert_eq!(web.urls().len(), 2, "asked about a song with timed lyrics");

    host.set_setting("online-lyrics", "prefer", Some(json!("always")));
    assert_eq!(
        find(5, &other, OwnLyrics::Timed).as_deref(),
        Some("[00:01.00] other")
    );
    assert_eq!(
        find(6, &track(), OwnLyrics::Untimed).as_deref(),
        Some("plain only")
    );
    assert_eq!(web.urls().len(), 4);
}

#[test]
fn online_lyrics_tries_again_when_lrclib_cant_be_reached() {
    let dirs = TestDirs::new();
    let web = FakeWeb::new(|_| Err("offline".into()));
    let discord = Arc::new(FakeDiscord::default());
    let (host, events) = bundled_host(&web, &discord, &dirs);
    for request in 1..=2 {
        host.find_lyrics(request, track(), OwnLyrics::Missing);
        assert_eq!(
            events.next_except_lyrics_search(),
            HostEvent::Lyrics {
                request,
                lyrics: None
            }
        );
    }
    assert_eq!(web.urls().len(), 2);
}

#[test]
fn discord_status_follows_the_player_and_its_settings() {
    let dirs = TestDirs::new();
    let web = FakeWeb::new(|_| ok("[]"));
    let discord = Arc::new(FakeDiscord::default());
    let (host, _events) = bundled_host(&web, &discord, &dirs);

    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(track()))));
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Playing));
    let wait_for = |check: &dyn Fn(&Option<mp_plugins::Activity>) -> bool| {
        let deadline = std::time::Instant::now() + common::TIMEOUT;
        loop {
            let last = discord.last();
            if check(&last) {
                return last;
            }
            assert!(std::time::Instant::now() < deadline, "last shown: {last:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let playing =
        wait_for(&|shown| shown.as_ref().is_some_and(|a| a.started_ms.is_some())).unwrap();
    assert_eq!(playing.details.as_deref(), Some("Song"));
    assert_eq!(playing.state.as_deref(), Some("Artist"));
    assert_eq!(playing.large_text.as_deref(), Some("Album"));
    assert_eq!(playing.large_image.as_deref(), Some("penguin"));
    assert_eq!(playing.status_display, StatusDisplay::Name);

    host.set_setting("discord-status", "artist", Some(json!(false)));
    host.set_setting("discord-status", "status", Some(json!("title")));
    let private = wait_for(&|shown| {
        shown
            .as_ref()
            .is_some_and(|a| a.status_display == StatusDisplay::Details)
    })
    .unwrap();
    assert_eq!(private.state, None);

    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Paused));
    wait_for(&Option::is_none);
    host.set_setting("discord-status", "paused", Some(json!(true)));
    let paused = wait_for(&Option::is_some).unwrap();
    assert_eq!(paused.state.as_deref(), Some("Paused"));
    assert_eq!(paused.started_ms, None);
}
