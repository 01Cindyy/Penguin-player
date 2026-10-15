//! The plugin host end to end: plugins on disk, events in, host events out.

mod common;

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use common::{TestDirs, track, write};
use mp_core::{PlaybackState, PlayerCommand, PlayerEvent, RepeatMode};
use mp_plugins::{CALLBACK_TIME_LIMIT, HostEvent, discover};

const SHORT_WAIT: Duration = Duration::from_millis(150);

#[test]
fn handlers_turn_events_into_commands() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "remote",
        r#"
        mp.on("track_changed", function(track)
          if track and track.title == "Song" then mp.player.next() end
        end)
        mp.on("state_changed", function(state)
          if state == "paused" then mp.player.play() end
        end)
        mp.on("seeked", function(ms) mp.player.seek(ms + 1000) end)
        mp.on("volume_changed", function(v)
          if v > 0.9 then mp.player.set_volume(0.5) end
        end)
        mp.on("shuffle_changed", function(on) mp.player.set_shuffle(not on) end)
        mp.on("repeat_changed", function(mode)
          if mode == "one" then mp.player.set_repeat("all") end
        end)
        mp.on("library_updated", function(count) mp.player.seek_by(-count) end)
        "#,
    );
    let (host, events) = dirs.start(&["remote"]);

    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(track()))));
    assert_eq!(events.command(), PlayerCommand::Next);
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Paused));
    assert_eq!(events.command(), PlayerCommand::Play);
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::from_millis(1500),
    });
    assert_eq!(
        events.command(),
        PlayerCommand::SeekTo(Duration::from_millis(2500))
    );
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 1.0,
        muted: false,
    });
    assert_eq!(
        events.command(),
        PlayerCommand::SetVolume {
            volume: 0.5,
            unmute: false,
        }
    );
    host.dispatch(PlayerEvent::ShuffleChanged(true));
    assert_eq!(events.command(), PlayerCommand::SetShuffle(false));
    host.dispatch(PlayerEvent::RepeatChanged(RepeatMode::One));
    assert_eq!(events.command(), PlayerCommand::SetRepeat(RepeatMode::All));
    host.dispatch(PlayerEvent::LibraryUpdated { track_count: 42 });
    assert_eq!(events.command(), PlayerCommand::SeekBy(-42));
    events.expect_none(SHORT_WAIT);
}

#[test]
fn every_player_command_is_available() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "commands",
        r#"
        local p = mp.player
        p.play() p.pause() p.toggle() p.stop() p.next() p.previous()
        p.seek(1500) p.seek_by(-250.4) p.set_volume(2) p.set_volume(-1)
        p.set_shuffle(true) p.set_repeat("one")
        local ok, err = pcall(p.set_repeat, "sometimes")
        mp.notify("bad repeat mode", err)
        "#,
    );
    let (_host, events) = dirs.start(&["commands"]);
    let expected = [
        PlayerCommand::Play,
        PlayerCommand::Pause,
        PlayerCommand::TogglePlayPause,
        PlayerCommand::Stop,
        PlayerCommand::Next,
        PlayerCommand::Previous,
        PlayerCommand::SeekTo(Duration::from_millis(1500)),
        PlayerCommand::SeekBy(-250),
        PlayerCommand::SetVolume {
            volume: 1.0,
            unmute: false,
        },
        PlayerCommand::SetVolume {
            volume: 0.0,
            unmute: false,
        },
        PlayerCommand::SetShuffle(true),
        PlayerCommand::SetRepeat(RepeatMode::One),
    ];
    for command in expected {
        assert_eq!(events.command(), command);
    }
    assert_eq!(
        events.notified("bad repeat mode"),
        r#"main.lua:6: unknown repeat mode 'sometimes' (use "off", "all" or "one")"#
    );
}

#[test]
fn getters_answer_from_the_latest_events() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "getters",
        r#"
        local function report()
          local p = mp.player
          local track = p.current_track()
          assert(p.repeat_mode() == p["repeat"]())
          mp.notify("state", string.format("%s|%.2f|%s|%s|%s|%d",
            p.state(), p.volume(), tostring(p.shuffle()), p.repeat_mode(),
            track and track.title or "none", p.position_ms()))
        end
        mp.on("library_updated", report)
        -- Measured from inside the plugin: the host may handle queued events back to back.
        mp.on("state_changed", function(state)
          if state == "playing" then mp.timer.after(100, report) end
        end)
        report()
        "#,
    );
    let (host, events) = dirs.start(&["getters"]);
    assert_eq!(events.notified("state"), "stopped|1.00|false|off|none|0");

    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(track()))));
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Paused));
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::from_secs(12),
    });
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.25,
        muted: false,
    });
    host.dispatch(PlayerEvent::ShuffleChanged(true));
    host.dispatch(PlayerEvent::RepeatChanged(RepeatMode::All));
    host.dispatch(PlayerEvent::LibraryUpdated { track_count: 1 });
    assert_eq!(events.notified("state"), "paused|0.25|true|all|Song|12000");

    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Playing));
    let body = events.notified("state");
    let position: u64 = body.rsplit('|').next().unwrap().parse().unwrap();
    assert!(body.starts_with("playing|"), "{body}");
    assert!(
        (12_100..15_000).contains(&position),
        "position should advance while playing: {body}"
    );
}

#[test]
fn track_tables_have_every_field() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "fields",
        r#"
        local keys = { "id", "title", "artist", "album", "album_artist", "genre", "year",
          "track_number", "disc_number", "duration_ms", "format", "sample_rate", "bit_depth",
          "channels", "bitrate_kbps", "path" }
        mp.on("track_changed", function(track)
          if track == nil then
            mp.notify("no track")
            return
          end
          local values = {}
          for _, key in ipairs(keys) do
            values[#values + 1] = key .. "=" .. tostring(track[key])
          end
          mp.notify("track", table.concat(values, " "))
          local same = mp.player.current_track()
          mp.notify("current", tostring(same.title == track.title and same.id == track.id))
        end)
        "#,
    );
    let (host, events) = dirs.start(&["fields"]);
    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(track()))));
    assert_eq!(
        events.notified("track"),
        "id=7 title=Song artist=Artist album=Album album_artist=Album Artist genre=Jazz \
         year=2024 track_number=1 disc_number=nil duration_ms=180000 format=flac \
         sample_rate=96000 bit_depth=24 channels=2 bitrate_kbps=nil \
         path=/music/Artist/Album/01 Song.flac"
    );
    assert_eq!(events.notified("current"), "true");
    host.dispatch(PlayerEvent::TrackChanged(None));
    assert_eq!(events.notification(), ("no track".to_owned(), None));
}

#[test]
fn notifications_reach_the_app() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "notes",
        r#"
        mp.notify("Hello", "World")
        mp.notify("Title only")
        mp.notify("Blank body", "   ")
        mp.notify(string.rep("é", 500))
        local ok = pcall(mp.notify, " ")
        mp.notify("empty title refused", tostring(not ok))
        mp.notify("plugin", mp.plugin.id .. "|" .. mp.plugin.name .. "|" .. mp.plugin.version
          .. "|" .. mp.api_version)
        "#,
    );
    write(
        &dirs.plugins().join("notes/plugin.json"),
        r#"{ "name": "Notes", "version": "2.1" }"#,
    );
    let (_host, events) = dirs.start(&["notes"]);
    match events.next() {
        HostEvent::Notify {
            plugin,
            title,
            body,
        } => {
            assert_eq!(plugin, "notes");
            assert_eq!(title, "Hello");
            assert_eq!(body.as_deref(), Some("World"));
        }
        other => panic!("expected a notification, got {other:?}"),
    }
    assert_eq!(events.notification(), ("Title only".to_owned(), None));
    assert_eq!(events.notification(), ("Blank body".to_owned(), None));
    let (long_title, _) = events.notification();
    assert_eq!(
        long_title.chars().count(),
        201,
        "200 characters and an ellipsis"
    );
    assert_eq!(events.notified("empty title refused"), "true");
    assert_eq!(events.notified("plugin"), "notes|Notes|2.1|2");
}

#[test]
fn logging_uses_the_plugin_target() {
    install_logger();
    let dirs = TestDirs::new();
    dirs.plugin(
        "chatty",
        r#"
        mp.log.debug("debugging")
        mp.log.warn("careful", 42, true, nil)
        print("printed", { } ~= nil)
        mp.notify("done")
        "#,
    );
    let (_host, events) = dirs.start(&["chatty"]);
    events.notified("done");
    let records = LOGGER.records_for("plugin::chatty");
    assert_eq!(
        records,
        [
            (log::Level::Debug, "debugging".to_owned()),
            (log::Level::Warn, "careful 42 true nil".to_owned()),
            (log::Level::Info, "printed true".to_owned()),
        ]
    );
}

#[test]
fn the_sandbox_holds_from_inside_a_plugin() {
    let dirs = TestDirs::new();
    let dir = dirs.plugin(
        "sandboxed",
        r#"
        local reachable = {}
        local function check(name, value)
          if value ~= nil then reachable[#reachable + 1] = name end
        end
        check("io", io) check("os.execute", os.execute) check("os.getenv", os.getenv)
        check("os.remove", os.remove) check("os.exit", os.exit) check("debug", debug)
        check("package", package) check("dofile", dofile) check("loadfile", loadfile)
        check("string.dump", string.dump)

        local escaped = pcall(require, "..secret") or pcall(require, "../secret")
        local chunk, message = load("\27Lua binary", "chunk", "b")
        local helper = require("lib.helper")

        mp.notify("sandbox", table.concat(reachable, ",") .. "|" .. tostring(escaped)
          .. "|" .. tostring(chunk) .. "|" .. tostring(message:find("binary") ~= nil)
          .. "|" .. helper.greet("penguin"))
        "#,
    );
    write(
        &dir.join("lib/helper.lua"),
        "return { greet = function(name) return 'hi ' .. name end }",
    );
    write(&dirs.plugins().join("secret.lua"), "return 'secret'");
    let (_host, events) = dirs.start(&["sandboxed"]);
    assert_eq!(events.notified("sandbox"), "|false|nil|true|hi penguin");
}

#[test]
fn runaway_handlers_are_stopped() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "spinner",
        r#"
        mp.on("seeked", function()
          while true do end
        end)
        mp.on("volume_changed", function(v) mp.notify("still alive", tostring(v)) end)
        "#,
    );
    let (host, events) = dirs.start(&["spinner"]);
    let started = Instant::now();
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (plugin, message) = events.error();
    assert_eq!(plugin, "spinner");
    assert!(
        message.contains("error in 'seeked' handler")
            && message.contains("main.lua:3")
            && message.contains("took longer than 250 ms"),
        "{message}"
    );
    assert!(started.elapsed() < CALLBACK_TIME_LIMIT * 4);
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    assert_eq!(events.notified("still alive"), "0.5");
}

#[test]
fn memory_bombs_are_stopped() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "hungry",
        r#"
        mp.on("seeked", function()
          local block = string.rep("x", 1024 * 1024)
          local hoard = {}
          for i = 1, 200 do hoard[i] = block .. i end
        end)
        mp.on("volume_changed", function() mp.notify("recovered") end)
        "#,
    );
    let (host, events) = dirs.start(&["hungry"]);
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (_, message) = events.error();
    assert!(message.contains("ran out of memory"), "{message}");
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    events.notified("recovered");
}

#[test]
fn storage_survives_reloads_and_restarts() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "counter",
        r#"
        local runs = (mp.storage.get("runs") or 0) + 1
        mp.storage.set("runs", runs)
        mp.storage.set("profile", { name = "Penguin", tags = { "a", "b" }, nested = { deep = true } })
        mp.storage.set("temporary", "gone soon")
        mp.storage.set("temporary", nil)
        local profile = mp.storage.get("profile")
        local refused = not pcall(mp.storage.set, "callback", function() end)
        mp.notify("runs", string.format("%d|%s|%s|%s|%s|%s", runs, profile.name, profile.tags[2],
          tostring(profile.nested.deep), tostring(mp.storage.get("temporary")), tostring(refused)))
        "#,
    );
    let (host, events) = dirs.start(&["counter"]);
    assert_eq!(events.notified("runs"), "1|Penguin|b|true|nil|true");
    host.reload();
    assert_eq!(events.notified("runs"), "2|Penguin|b|true|nil|true");
    drop(host);

    let (_host, events) = dirs.start(&["counter"]);
    assert_eq!(events.notified("runs"), "3|Penguin|b|true|nil|true");
    let saved = std::fs::read_to_string(dirs.data().join("counter.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["runs"], 3);
    assert_eq!(saved["profile"]["tags"], serde_json::json!(["a", "b"]));
    assert!(saved.get("temporary").is_none());
}

#[test]
fn timers_fire_and_cancel() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "clock",
        r#"
        local ticks = 0
        local cancelled = mp.timer.after(30, function() mp.notify("cancelled timer fired") end)
        mp.notify("cancel", tostring(mp.timer.cancel(cancelled)) .. "|"
          .. tostring(mp.timer.cancel(cancelled)))
        mp.timer.after(40, function() mp.notify("once") end)
        local every
        every = mp.timer.every(10, function()
          ticks = ticks + 1
          if ticks == 3 then
            mp.timer.cancel(every)
            mp.notify("every", tostring(ticks))
          end
        end)
        mp.timer.after(200, function() mp.notify("done", tostring(ticks)) end)
        "#,
    );
    let started = Instant::now();
    let (_host, events) = dirs.start(&["clock"]);
    assert_eq!(events.notified("cancel"), "true|false");
    let mut titles: Vec<String> = (0..2).map(|_| events.notification().0).collect();
    titles.sort();
    assert_eq!(titles, ["every", "once"]);
    assert_eq!(events.notified("done"), "3");
    assert!(started.elapsed() >= Duration::from_millis(200));
    events.expect_none(SHORT_WAIT);
}

#[test]
fn plugins_can_be_enabled_and_disabled_at_runtime() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "toggle",
        r#"
        mp.notify("loaded")
        mp.on("volume_changed", function() mp.notify("volume") end)
        mp.on("shutdown", function() mp.notify("bye") end)
        "#,
    );
    let (host, events) = dirs.start(&[]);
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    events.expect_none(SHORT_WAIT);

    host.set_enabled("toggle", true);
    events.notified("loaded");
    events.notified("volume");
    host.set_enabled("toggle", true);
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    events.notified("volume");

    host.set_enabled("toggle", false);
    events.notified("bye");
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    host.set_enabled("not-installed", true);
    host.set_enabled("../escape", true);
    events.expect_none(SHORT_WAIT);

    host.set_enabled("toggle", true);
    events.notified("loaded");
    events.notified("volume");
    host.reload();
    events.notified("bye");
    events.notified("loaded");
    events.notified("volume");
    events.expect_none(SHORT_WAIT);
}

/// A plugin switched on hears what the player is doing; the plugins already running
/// don't hear it again, and the playing position carries on.
#[test]
fn only_plugins_switched_on_hear_the_current_state() {
    let dirs = TestDirs::new();
    let script = |name: &str| {
        format!(
            r#"
            mp.on("track_changed", function(track)
              mp.notify("{name}", track.title .. "|" .. mp.player.position_ms())
            end)
            "#
        )
    };
    dirs.plugin("first", &script("first"));
    dirs.plugin("second", &script("second"));
    let (host, events) = dirs.start(&["first"]);
    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(track()))));
    assert_eq!(events.notified("first"), "Song|0");
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::from_secs(120),
    });

    host.set_enabled("second", true);
    assert_eq!(events.notified("second"), "Song|120000");
    events.expect_none(SHORT_WAIT);
}

#[test]
fn repeatedly_failing_plugins_are_unloaded() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "flaky",
        r#"
        mp.on("seeked", function(ms)
          if ms > 0 then error("broken handler") end
        end)
        mp.on("shutdown", function() mp.notify("shutdown ran") end)
        "#,
    );
    let (host, events) = dirs.start(&["flaky"]);
    let seek = |ms| {
        host.dispatch(PlayerEvent::Seeked {
            position: Duration::from_millis(ms),
        })
    };
    let fail = |times: usize| {
        for _ in 0..times {
            seek(1);
            let (_, message) = events.error();
            assert_eq!(
                message,
                "error in 'seeked' handler: main.lua:3: broken handler"
            );
        }
    };

    fail(9);
    seek(0);
    fail(9);
    events.expect_none(SHORT_WAIT);

    fail(1);
    let (plugin, message) = events.error();
    assert_eq!(plugin, "flaky");
    assert!(
        message.contains("stopped after 10 errors in a row"),
        "{message}"
    );
    seek(1);
    events.expect_none(SHORT_WAIT);
}

#[test]
fn broken_plugins_report_their_problem() {
    let dirs = TestDirs::new();
    dirs.plugin_with_manifest("bad-json", "{ name: ", "");
    dirs.plugin_with_manifest("future", r#"{ "name": "Future", "api_version": 99 }"#, "");
    dirs.plugin("syntax", "mp.on(\"seeked\", function()\n  end end");
    dirs.plugin("crashes", "local x = nil\nx.y = 1");
    dirs.plugin("typo", r#"mp.on("track_change", function() end)"#);

    let problems: Vec<(String, bool)> = discover(&dirs.plugins())
        .into_iter()
        .filter(|info| !info.is_bundled())
        .map(|info| (info.id, info.problem.is_some()))
        .collect();
    assert_eq!(
        problems,
        [
            ("bad-json".to_owned(), true),
            ("crashes".to_owned(), false),
            ("future".to_owned(), true),
            ("syntax".to_owned(), false),
            ("typo".to_owned(), false),
        ]
    );

    let (_host, events) = dirs.start(&["bad-json", "future", "syntax", "crashes", "typo"]);
    let mut errors: Vec<(String, String)> = (0..5).map(|_| events.error()).collect();
    errors.sort();
    let message = |id: &str| &errors.iter().find(|(plugin, _)| plugin == id).unwrap().1;
    assert!(message("bad-json").contains("plugin.json is invalid"));
    assert!(message("future").contains("API version 99"));
    assert!(
        message("syntax").starts_with("failed to load: main.lua:2:"),
        "{}",
        message("syntax")
    );
    assert_eq!(
        message("crashes"),
        "failed to load: main.lua:2: attempt to index a nil value (local 'x')"
    );
    assert!(
        message("typo").contains("main.lua:1: unknown event 'track_change'"),
        "{}",
        message("typo")
    );
}

#[test]
fn dropping_the_host_runs_shutdown_handlers_without_hanging() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "stubborn",
        r#"mp.on("shutdown", function() while true do end end)"#,
    );
    dirs.plugin(
        "polite",
        r#"mp.on("shutdown", function() mp.notify("bye", mp.player.state()) end)"#,
    );
    let (host, events) = dirs.start(&["stubborn", "polite"]);
    std::thread::sleep(SHORT_WAIT);

    let started = Instant::now();
    drop(host);
    assert!(
        started.elapsed() < CALLBACK_TIME_LIMIT * 4,
        "took {:?}",
        started.elapsed()
    );
    let (plugin, message) = events.error();
    assert_eq!(plugin, "stubborn");
    assert!(message.contains("took longer than"), "{message}");
    assert_eq!(events.notified("bye"), "stopped");
    assert!(events.0.is_closed(), "the host thread has finished");
}

#[test]
fn pathological_library_calls_are_stopped_too() {
    let dirs = TestDirs::new();
    // In stock Lua this single call would backtrack for hours inside C code.
    dirs.plugin(
        "backtracker",
        r#"
        mp.on("seeked", function()
          string.find(string.rep("a", 5000), ".-.-.-.-.-.-.-b")
        end)
        mp.on("volume_changed", function() mp.notify("still alive") end)
        "#,
    );
    let (host, events) = dirs.start(&["backtracker"]);
    let started = Instant::now();
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (_, message) = events.error();
    assert!(
        message.contains("main.lua:3: took longer than 250 ms"),
        "{message}"
    );
    assert!(started.elapsed() < CALLBACK_TIME_LIMIT * 4);
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    events.notified("still alive");
}

#[test]
fn all_handlers_of_an_event_share_one_time_limit() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "many",
        r#"
        for i = 1, 100 do
          mp.on("seeked", function()
            local start = os.clock()
            while os.clock() - start < 0.05 do end
          end)
        end
        mp.on("volume_changed", function() mp.notify("still alive") end)
        "#,
    );
    let (host, events) = dirs.start(&["many"]);
    let started = Instant::now();
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (_, message) = events.error();
    assert!(message.contains("took longer than 250 ms"), "{message}");
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    events.notified("still alive");
    assert!(
        started.elapsed() < CALLBACK_TIME_LIMIT * 4,
        "100 handlers of 50 ms each must not run for 5 s: {:?}",
        started.elapsed()
    );
}

#[test]
fn floods_of_notifications_and_commands_are_capped() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "flood",
        r#"
        for i = 1, 1000 do mp.notify("note " .. i) end
        for i = 1, 1000 do mp.player.next() end
        for i = 1, 1000 do pcall(error, "caught errors are not reported") end
        "#,
    );
    let (_host, events) = dirs.start(&["flood"]);
    let (mut notifications, mut commands) = (0, 0);
    while let Some(event) = events.try_next(SHORT_WAIT) {
        match event {
            HostEvent::Notify { .. } => notifications += 1,
            HostEvent::Command { .. } => commands += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(notifications, 10);
    assert_eq!(commands, 50);
}

#[test]
fn floods_of_log_messages_are_capped() {
    install_logger();
    let dirs = TestDirs::new();
    dirs.plugin(
        "logorrhea",
        r#"
        for i = 1, 1000 do print("line " .. i) end
        for i = 1, 1000 do mp.log.error("error " .. i) end
        mp.notify("done")
        "#,
    );
    let (_host, events) = dirs.start(&["logorrhea"]);
    events.notified("done");
    let records = LOGGER.records_for("plugin::logorrhea");
    let lines = records
        .iter()
        .filter(|(_, message)| message.starts_with("line "))
        .count();
    assert_eq!(lines, 100);
    assert!(records.len() < 110, "{} records", records.len());
}

#[test]
fn huge_error_messages_are_shortened() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "shouty",
        r#"mp.on("seeked", function() error(string.rep("x", 4 * 1024 * 1024)) end)"#,
    );
    let (host, events) = dirs.start(&["shouty"]);
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (_, message) = events.error();
    assert!(message.starts_with("error in 'seeked' handler: main.lua:1: xxx"));
    assert!(
        message.chars().count() < 1100,
        "{} characters",
        message.len()
    );
}

#[test]
fn error_objects_with_runaway_tostring_are_stopped() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "sneaky",
        r#"
        mp.on("seeked", function()
          error(setmetatable({}, { __tostring = function() while true do end end }))
        end)
        mp.on("volume_changed", function() mp.notify("still alive") end)
        "#,
    );
    let (host, events) = dirs.start(&["sneaky"]);
    let started = Instant::now();
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    events.error();
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.5,
        muted: false,
    });
    events.notified("still alive");
    assert!(started.elapsed() < CALLBACK_TIME_LIMIT * 4);
}

#[test]
fn busy_plugins_never_hold_up_the_app() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "slow",
        r#"
        mp.on("seeked", function()
          local start = os.clock()
          while os.clock() - start < 0.2 do end
        end)
        "#,
    );
    let (host, events) = dirs.start(&["slow"]);
    let started = Instant::now();
    for _ in 0..20 {
        host.dispatch(PlayerEvent::Seeked {
            position: Duration::ZERO,
        });
    }
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "dispatch must not wait"
    );

    // Twenty queued 200 ms handlers would take four seconds; dropping skips them.
    drop(host);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "took {:?}",
        started.elapsed()
    );
    events.expect_none(Duration::ZERO);
}

/// Nesting `gsub` through its replacement function is stopped with an error before it
/// runs out of the host thread's stack, which would abort the whole app.
#[test]
fn deep_nesting_through_library_functions_is_an_error_not_a_crash() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "nested",
        r#"
        local function nest(s) return (s:gsub(".", function(c) return nest(c) end)) end
        local lookup
        lookup = setmetatable({}, { __index = function(_, k) return (k:gsub(".", lookup)) end })
        local _, by_function = pcall(nest, "a")
        local _, by_table = pcall(string.gsub, "a", ".", lookup)
        mp.notify("nested", by_function .. "|" .. by_table)
        "#,
    );
    dirs.plugin(
        "bystander",
        r#"mp.on("seeked", function() mp.notify("alive") end)"#,
    );
    let (host, events) = dirs.start(&["nested", "bystander"]);
    assert_eq!(
        events.notified("nested"),
        "main.lua:2: C stack overflow|main.lua:4: C stack overflow"
    );
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    events.notified("alive");
}

/// `mp.timer.after(1e300, f)` and `mp.player.seek(1e300)` are clamped, not a panic of
/// the host thread that would stop every plugin.
#[test]
fn huge_and_signed_zero_durations_are_accepted() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "extreme",
        r#"
        mp.timer.after(1e300, function() mp.notify("too early") end)
        mp.timer.every(1e300, function() mp.notify("too early") end)
        mp.timer.after(-0.0, function() mp.notify("zero") end)
        mp.player.seek(1e300)
        "#,
    );
    let (_host, events) = dirs.start(&["extreme"]);
    assert_eq!(
        events.command(),
        PlayerCommand::SeekTo(Duration::from_secs(30 * 24 * 60 * 60))
    );
    events.notified("zero");
    events.expect_none(SHORT_WAIT);
}

/// `mp` functions check the time limit themselves: Lua only checks it every thousand
/// instructions, which a loop of slow calls would take seconds to reach.
#[test]
fn loops_of_slow_api_calls_are_stopped_in_time() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "saver",
        r#"
        local big = string.rep("x", 900 * 1024)
        mp.on("seeked", function()
          while true do mp.storage.set("big", big) end
        end)
        "#,
    );
    let (host, events) = dirs.start(&["saver"]);
    let started = Instant::now();
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (_, message) = events.error();
    assert!(
        message.contains("main.lua:4: took longer than 250 ms"),
        "{message}"
    );
    assert!(
        started.elapsed() < CALLBACK_TIME_LIMIT * 4,
        "took {:?}",
        started.elapsed()
    );
}

/// Only values the JSON reader accepts are saved, so the next start never discards
/// the whole store as unreadable.
#[test]
fn stored_values_can_always_be_read_back() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "nesting",
        r#"
        local function nested(depth)
          local value = "core"
          for _ = 1, depth do value = { value } end
          return value
        end
        if mp.storage.get("kept") == nil then
          mp.storage.set("kept", "yes")
          mp.storage.set("deep", nested(64))
          local ok, err = pcall(mp.storage.set, "deeper", nested(65))
          mp.notify("first run", tostring(ok) .. "|" .. err)
        else
          local value, depth = mp.storage.get("deep"), 0
          while type(value) == "table" do value, depth = value[1], depth + 1 end
          mp.notify("second run", mp.storage.get("kept") .. "|" .. depth .. "|" .. value)
        end
        "#,
    );
    let (host, events) = dirs.start(&["nesting"]);
    let body = events.notified("first run");
    assert!(
        body.starts_with("false|main.lua:10: can't store 'deeper'")
            && body.contains("nested at most 64 deep"),
        "{body}"
    );
    drop(host);
    let (_host, events) = dirs.start(&["nesting"]);
    assert_eq!(events.notified("second run"), "yes|64|core");
}

#[test]
fn event_handlers_are_limited() {
    let dirs = TestDirs::new();
    dirs.plugin(
        "greedy",
        r#"
        for _ = 1, 256 do mp.on("seeked", function() end) end
        local ok, err = pcall(mp.on, "track_changed", function() end)
        mp.notify("limit", tostring(ok) .. "|" .. err)
        "#,
    );
    let (_host, events) = dirs.start(&["greedy"]);
    assert_eq!(
        events.notified("limit"),
        "false|main.lua:3: a plugin can register at most 256 event handlers"
    );
}

/// Once the app is quitting, an event being handled skips the plugins it hasn't
/// reached yet.
#[test]
fn quitting_skips_the_remaining_plugins_of_an_event() {
    let dirs = TestDirs::new();
    let ids: Vec<String> = (0..8).map(|i| format!("slow-{i}")).collect();
    for id in &ids {
        dirs.plugin(
            id,
            r#"
            mp.on("seeked", function()
              local start = os.clock()
              while os.clock() - start < 0.2 do end
            end)
            "#,
        );
    }
    let enabled: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (host, _events) = dirs.start(&enabled);
    std::thread::sleep(SHORT_WAIT);
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    drop(host);
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "eight 200 ms handlers must not all run while quitting: {:?}",
        started.elapsed()
    );
}

#[test]
fn the_host_handle_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<mp_plugins::PluginHost>();
    assert_send_sync::<HostEvent>();
}

/// Records log messages so tests can check what plugins logged.
struct CaptureLogger {
    records: Mutex<Vec<(String, log::Level, String)>>,
}

static LOGGER: CaptureLogger = CaptureLogger {
    records: Mutex::new(Vec::new()),
};

impl CaptureLogger {
    fn records_for(&self, target: &str) -> Vec<(log::Level, String)> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|(record_target, _, _)| record_target == target)
            .map(|(_, level, message)| (*level, message.clone()))
            .collect()
    }
}

impl log::Log for CaptureLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        self.records.lock().unwrap().push((
            record.target().to_owned(),
            record.level(),
            record.args().to_string(),
        ));
    }

    fn flush(&self) {}
}

fn install_logger() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        log::set_logger(&LOGGER).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Trace);
    });
}
