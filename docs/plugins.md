# Writing plugins

Plugins add small features to the player: a notification when a track starts, a sleep
timer, lyrics from the internet, your music on Discord, playback rules of your own. They
are written in [Lua 5.4](https://www.lua.org/manual/5.4/) and run inside the app.

Every plugin runs in its own sandbox with strict time and memory limits. A plugin that
crashes, loops forever or eats memory is stopped and reported; it can never freeze the
app or touch your files. Plugins are offline unless they ask for more: a plugin can ask
to connect to the websites it names, or to show what's playing on Discord, and the user
allows that when switching it on (see [Permissions](#permissions)).

- [Your first plugin](#your-first-plugin)
- [Plugin folder layout](#plugin-folder-layout)
- [plugin.json reference](#pluginjson-reference)
- [Permissions](#permissions)
- [Settings](#settings)
- [How plugins run](#how-plugins-run)
- [Lua API reference](#lua-api-reference)
- [Limits](#limits)
- [The sandbox](#the-sandbox)
- [Errors and debugging](#errors-and-debugging)
- [Included plugins](#included-plugins)
- [Example plugins](#example-plugins)

## Your first plugin

1. Open the plugins folder. It lives in the app's config folder:

   | System | Plugins folder |
   |---|---|
   | Linux | `~/.config/penguin-player/plugins/` (or `$XDG_CONFIG_HOME/penguin-player/plugins/`) |
   | Portable mode (`PENGUIN_PLAYER_HOME=<dir>`) | `<dir>/config/plugins/` |

2. Create a folder for the plugin. **The folder name is the plugin's id**, so keep it
   short and lowercase, for example `hello`.

3. Add `hello/plugin.json`:

   ```json
   {
     "name": "Hello",
     "version": "1.0.0",
     "description": "Says hello when a song starts."
   }
   ```

4. Add `hello/main.lua`:

   ```lua
   mp.on("track_changed", function(track)
     if track then
       mp.notify("Hello!", "Now playing " .. track.title)
     end
   end)
   ```

5. Switch the plugin on in the app's plugin settings. Plugins are off until you enable
   them; the app remembers enabled plugins by id (`enabled_plugins` in `settings.json`).

Play a song and the notification appears. After editing a plugin, reload plugins (or
restart the app) to pick up the changes.

## Plugin folder layout

```
plugins/
  hello/                 the folder name is the plugin id
    plugin.json          required: name, version, ...
    main.lua             the entry script (name configurable)
    lib/
      format.lua         optional modules, loaded with require("lib.format")
```

Hidden folders (names starting with `.`) are ignored. Everything a plugin loads must be
inside its own folder.

## plugin.json reference

| Field | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `name` | string | yes | | Name shown in the plugin manager. Must not be empty. |
| `version` | string | no | `""` | Your plugin's version, free form (`"1.2.0"`). |
| `author` | string | no | `""` | Who made it. |
| `description` | string | no | `""` | One sentence shown in the plugin manager. |
| `entry` | string | no | `"main.lua"` | The script to run, relative to the plugin folder. |
| `api_version` | integer | no | current version | The plugin API version the plugin was written for. |
| `permissions` | object | no | none | What the plugin does beyond the sandbox: `network` (a list of sites) and `discord` (`true`). See [Permissions](#permissions). |
| `privacy` | string | no | `""` | What the plugin sends where, in your words. Shown when the user is asked to allow its permissions. |
| `settings` | list | no | `[]` | The settings on the plugin's settings page. See [Settings](#settings). |

The app lists every plugin folder, including broken ones, so you can see what is wrong.
A plugin is refused, with an explanation, when:

- `plugin.json` is missing or isn't valid JSON,
- `name` is empty,
- the entry script doesn't exist or points outside the plugin folder,
- `api_version` is newer than the app supports (the current version is **2**),
- a site in `permissions` isn't a plain lowercase site name such as `"lrclib.net"`
  (no `https://`, port or path), or there are more than 16,
- a setting is malformed (see [Settings](#settings)), or there are more than 32,
- its folder has the same name as one of the [included plugins](#included-plugins).

## Permissions

A plugin that needs more than the sandbox says so in `plugin.json`:

```json
{
  "name": "Lyrics Finder",
  "api_version": 2,
  "permissions": {
    "network": ["lrclib.net"],
    "discord": true
  },
  "privacy": "Only the title and artist of songs without lyrics are sent."
}
```

| Permission | Lets the plugin use |
|---|---|
| `"network": ["site", ...]` | [`mp.http`](#web-requests-mphttp), for HTTPS requests to exactly these sites. `lrclib.net` doesn't cover `www.lrclib.net`; list both if you need both. |
| `"discord": true` | [`mp.discord`](#discord-mpdiscord), to show what's playing on the user's Discord profile. |

When the user switches such a plugin on, the app asks first, naming the sites and what
the plugin may do, followed by your `privacy` text. A plugin only gets what it asks for
and the user allowed: if a new version asks for more, it is switched off until the user
switches it on again and allows it.

## Settings

Plugins with `settings` in `plugin.json` get a settings page, opened from the gear
button next to their switch. The app saves the user's choices and the plugin reads them
with [`mp.settings.get(key)`](#settings-mpsettings).

```json
"settings": [
  { "key": "loud", "type": "boolean", "title": "Loud Notifications", "default": true },
  { "key": "minutes", "type": "number", "title": "Minutes", "subtitle": "Until it pauses",
    "default": 30, "min": 1, "max": 600, "step": 5 },
  { "key": "style", "type": "choice", "title": "Style", "default": "short",
    "choices": [ { "value": "short", "title": "Short" }, { "value": "long", "title": "Long" } ] }
]
```

| Field | Types | Meaning |
|---|---|---|
| `key` | all | The name the plugin reads it by: letters, digits, `_` and `-`. Unique within the plugin. |
| `type` | all | `"boolean"` (a switch), `"number"` (a number field) or `"choice"` (a drop-down). |
| `title` | all | The setting's name. Must not be empty. |
| `subtitle` | all | Optional explanation under the title. |
| `default` | all | The value until the user changes it: `true`/`false` (optional, `false` if missing), a number from `min` to `max`, or one of the choices' `value`s. |
| `min`, `max` | number | The range, `min <= default <= max`. |
| `step` | number | How much the arrows change it (default `1`, above `0`). Whole steps give whole numbers. |
| `choices` | choice | The options, each with a `value` the plugin sees and a `title` the user sees. Values must differ. |

## How plugins run

- All plugins share one background thread, and each has its own Lua state. Plugins can't
  see each other's variables.
- When a plugin is switched on, its entry script runs once from top to bottom. That is
  where you register event handlers, start timers and read saved settings.
- Afterwards the plugin only runs when something happens: an event it subscribed to, a
  timer firing, or the app shutting it down.
- Plugins never block the app. Events are queued for the plugin thread, and commands a
  plugin sends (such as `mp.player.pause()`) are carried out by the app shortly after.
- Each call into a plugin (an event, a round of timers, a web request's answer) is
  limited to 250 ms of running time, and each plugin to 32 MiB of memory (see
  [Limits](#limits)).
- Web requests run in the background; their answers arrive later, like events.

## Lua API reference

Everything lives in the global table `mp`.

### `mp.api_version` and `mp.plugin`

| Value | Type | Meaning |
|---|---|---|
| `mp.api_version` | integer | The API version of the running app (currently `2`). |
| `mp.plugin.id` | string | The plugin's id (its folder name). |
| `mp.plugin.name` | string | `name` from `plugin.json`. |
| `mp.plugin.version` | string | `version` from `plugin.json`. |

### Events: `mp.on(event, handler)`

Calls `handler` every time `event` happens. You can register several handlers for the
same event; they run in the order they were added. Unknown event names are an error, so
typos are caught when the plugin loads. A plugin can register up to 256 handlers.

| Event | Handler arguments | When |
|---|---|---|
| `track_changed` | `track` (a [track table](#track-tables)) or `nil` | A different track became current; `nil` when nothing is loaded. |
| `state_changed` | `state`: `"playing"`, `"paused"` or `"stopped"` | Playback started, paused or stopped. |
| `seeked` | `position_ms` (integer) | The user or a plugin jumped to a new position. |
| `volume_changed` | `volume` (number, `0.0` to `1.0`), `muted` (boolean) | The volume level changed, or the user muted or unmuted. Muting keeps the level. |
| `shuffle_changed` | `enabled` (boolean) | Shuffle was switched on or off. |
| `repeat_changed` | `mode`: `"off"`, `"all"` or `"one"` | The repeat mode changed. |
| `library_updated` | `track_count` (integer) | The music library was scanned or changed. |
| `setting_changed` | `key` (string), `value` | The user changed one of the plugin's [settings](#settings). |
| `shutdown` | none | The plugin is about to be unloaded (app exit, switched off or reloaded). Save state here. When the app is quitting, only volume, shuffle and repeat changes still take effect. |

```lua
mp.on("state_changed", function(state)
  mp.log.info("playback is now " .. state)
end)
```

### Track tables

Tracks are plain tables. Text fields are empty strings (`""`) when the file has no such
tag; number fields are `nil` when unknown.

| Field | Type | Example |
|---|---|---|
| `id` | integer | `42` (stable while the file keeps its path) |
| `title` | string | `"So What"` (never empty: falls back to the file name) |
| `artist` | string | `"Miles Davis"` |
| `album` | string | `"Kind of Blue"` |
| `album_artist` | string | `""` |
| `genre` | string | `"Jazz"` |
| `year` | integer or nil | `1959` |
| `track_number` | integer or nil | `1` |
| `disc_number` | integer or nil | `nil` |
| `duration_ms` | integer | `562000` |
| `format` | string | `"flac"`, `"alac"`, `"wav"`, `"aiff"`, `"mp3"`, `"aac"`, `"opus"`, `"vorbis"` or `"unknown"` |
| `sample_rate` | integer or nil | `96000` (Hz) |
| `bit_depth` | integer or nil | `24` (lossless formats only) |
| `channels` | integer or nil | `2` |
| `bitrate_kbps` | integer or nil | `320` |
| `path` | string | `"/home/me/Music/Miles Davis/Kind of Blue/01 So What.flac"` |

### Controlling playback: `mp.player`

Commands are sent to the app, which carries them out exactly as if you had pressed the
button. The resulting change arrives back as an event (for example `state_changed`).

| Function | Effect |
|---|---|
| `mp.player.play()` | Start or resume playback. |
| `mp.player.pause()` | Pause. |
| `mp.player.toggle()` | Play if paused, pause if playing. |
| `mp.player.stop()` | Stop. |
| `mp.player.next()` | Skip to the next track. |
| `mp.player.previous()` | Go to the previous track. |
| `mp.player.seek(ms)` | Jump to `ms` milliseconds into the track (`ms >= 0`). |
| `mp.player.seek_by(ms)` | Jump forwards by `ms` milliseconds, or backwards when negative. |
| `mp.player.set_volume(v)` | Set the volume level, `0.0` to `1.0` (values outside are clamped). Muting is the user's: a muted player stays muted and plays at this level once unmuted. |
| `mp.player.set_shuffle(on)` | Switch shuffle on (`true`) or off (`false`). |
| `mp.player.set_repeat(mode)` | Set repeat to `"off"`, `"all"` or `"one"`. |

The getters answer instantly from what the plugin host has seen so far:

| Function | Returns |
|---|---|
| `mp.player.state()` | `"playing"`, `"paused"` or `"stopped"` |
| `mp.player.current_track()` | the current [track table](#track-tables), or `nil` |
| `mp.player.volume()` | number, `0.0` to `1.0`: the volume level, also while muted |
| `mp.player.muted()` | boolean |
| `mp.player.shuffle()` | boolean |
| `mp.player.repeat_mode()` | `"off"`, `"all"` or `"one"` |
| `mp.player.position_ms()` | integer: the playback position, estimated from the last known position and the time played since |

`repeat` is a reserved word in Lua, so `mp.player.repeat()` doesn't parse. Use
`mp.player.repeat_mode()`, or the equivalent `mp.player["repeat"]()`.

### Notifications: `mp.notify(title, body)`

Shows a small notification inside the app. `title` is required; `body` is optional.
Long texts are shortened (200 characters for the title, 2000 for the body). A plugin can
show 10 notifications at once and 2 per second after that; extra ones are dropped.

```lua
mp.notify("Sleep timer", "Pausing in 10 minutes")
```

### Logging: `mp.log`

`mp.log.debug(...)`, `mp.log.info(...)`, `mp.log.warn(...)` and `mp.log.error(...)` write
to the app's log, tagged with your plugin's id (`plugin::<id>`). Arguments are joined
with spaces like `print`, and `print(...)` itself is the same as `mp.log.info(...)`.

```lua
mp.log.warn("unexpected format", track.format)
```

### Saving data: `mp.storage`

A small key/value store that survives restarts and plugin reloads, saved as JSON in the
app's data folder (on Linux `~/.local/share/penguin-player/plugin-data/<id>.json`). Changes
are written to disk safely, at most once a second; whatever is still waiting is written
when the plugin is switched off, reloaded or the app quits.

| Function | Meaning |
|---|---|
| `mp.storage.get(key)` | The value saved under `key`, or `nil`. |
| `mp.storage.set(key, value)` | Saves `value` under `key`. Setting `nil` deletes the key. |

Keys are non-empty strings. Values can be `nil`, booleans, numbers, strings and tables
made of those: lists (`{ "a", "b" }`) or tables with string keys (`{ name = "x" }`),
nested up to 64 levels deep. Functions, deeper nesting and tables that mix list items
with named keys are refused with an error. The whole store is limited to 1 MiB.

Since the store is a plain JSON file, it is also a handy place for settings the user can
edit by hand; see the [sleep timer](#example-plugins) example.

### Timers: `mp.timer`

| Function | Meaning |
|---|---|
| `mp.timer.after(ms, fn)` | Calls `fn()` once, `ms` milliseconds from now. Returns the timer's id. |
| `mp.timer.every(ms, fn)` | Calls `fn()` every `ms` milliseconds (at least 10) until cancelled. Returns the timer's id. |
| `mp.timer.cancel(id)` | Stops a timer. Returns `true` if it was still pending. |

Delays can be fractional and are capped at 30 days. If the plugin thread was busy and a
repeating timer missed some runs, it runs once and continues at its normal pace rather
than catching up in a burst. A plugin can have up to 256 timers at once.

```lua
local ticks = 0
local id
id = mp.timer.every(1000, function()
  ticks = ticks + 1
  if ticks == 5 then
    mp.timer.cancel(id)
  end
end)
```

### Settings: `mp.settings`

`mp.settings.get(key)` returns the user's choice for one of the plugin's
[settings](#settings), or its default: a boolean, a number or a choice's `value`. A key
that `plugin.json` doesn't have is an error. Changes arrive as `setting_changed` events.

```lua
local function loud()
  return mp.settings.get("loud")
end
mp.on("setting_changed", function(key, value)
  mp.log.info(key, "is now", value)
end)
```

### Web requests: `mp.http`

Only for plugins with [`"network"` permission](#permissions).

`mp.http.get(url, [options], callback)` starts an HTTPS `GET` request and returns at
once. When it finishes, `callback(response)` runs, or `callback(nil, message)` if it
failed (no connection, timeout, too large).

| Option | Meaning |
|---|---|
| `query` | A table of names and values (text or numbers) added to the URL, encoded for you. |
| `headers` | A table of request headers. `User-Agent` (which names the app) and `Host` are set by the app. |

| Response field | Meaning |
|---|---|
| `status` | The HTTP status, such as `200` or `404`. Error statuses are responses, not failures. |
| `ok` | `true` for a 2xx status. |
| `body` | The response body as a string. |

The URL must start with `https://` and name one of the plugin's sites, without a port or
user name. Redirects are not followed. Requests that break these rules, or the
[limits](#limits), raise an error.

```lua
mp.http.get("https://lrclib.net/api/search",
  { query = { track_name = "So What", artist_name = "Miles Davis" } },
  function(response, err)
    if not response then
      mp.log.warn("search failed:", err)
    elseif response.ok then
      local results = mp.json.decode(response.body)
      mp.log.info(#results, "results")
    end
  end)
```

### JSON: `mp.json`

`mp.json.decode(text)` turns JSON into Lua values: objects become tables with string
keys, arrays become lists, and `null` becomes `nil`. Invalid JSON raises an error, so
wrap it in `pcall` when the text comes from the internet.

### Lyrics: `mp.lyrics`

`mp.lyrics.provide(finder, [options])` makes the plugin a source of lyrics. When Now
Playing shows the lyrics of a song that has none of its own (in its tags, an `.lrc` file
beside it, or found before), the app calls `finder(track, done)` with the song's
[track table](#track-tables). Call `done(text)` with the lyrics, as
[LRC](https://en.wikipedia.org/wiki/LRC_(file_format)) (timed) or plain text, or
`done(nil)` if you found none. `done` may be called later, such as from a web request's
callback; only the first call counts.

To replace lyrics songs have of their own too, set `options.replace`:

| `replace` | The finder is asked about songs |
|---|---|
| `"none"` (the default) | without lyrics |
| `"untimed"` | without lyrics or with untimed ones |
| `"all"` | all of them |

The finder then gets `track.lyrics`: `"timed"` or `"untimed"` for the song's own lyrics,
or `nil` if it has none. Untimed lyrics never replace timed ones; the app treats them as
none found. Call `mp.lyrics.provide` again to change the finder or its options, such as
when a setting changes.

With several lyrics plugins, the first lyrics found are shown. The app keeps lyrics it
was given, so it asks only once per song. A finder that raises an error counts as having
found none, and the app stops waiting after 20 seconds.

```lua
mp.lyrics.provide(function(track, done)
  if track.title == "Silence" then
    done("[00:00.00] ...")
  else
    done(nil)
  end
end)
```

### Discord: `mp.discord`

Only for plugins with [`"discord"` permission](#permissions). Shows an activity on the
user's Discord profile while the Discord app (or Vesktop) runs on the same computer;
otherwise nothing happens. The app sends at most one update every four seconds, the
newest, and clears the activity when the plugin stops.

`mp.discord.set_activity(activity)` shows `activity`, a table with any of:

| Field | Meaning |
|---|---|
| `type` | `"listening"` (the default), `"playing"`, `"watching"` or `"competing"`. |
| `details` | The first line, such as the song's title. |
| `state` | The second line, such as the artist. |
| `large_image` | The big picture: an image uploaded to the app's Discord application, by name (the app's icon is `"penguin"`), or an `https://` address. Covers on the computer can't be shown. |
| `large_text` | Shown when pointing at the big picture. |
| `small_image`, `small_text` | The same for the small picture in the corner. |
| `elapsed_ms`, `duration_ms` | How far into the song it is and how long it is, for Discord's progress bar. |
| `status_display` | What member lists show after "Listening to": `"name"` (the app's name, the default), `"state"` or `"details"`. |

Texts are cut at 128 characters. `mp.discord.clear()` removes the activity.

### Modules: `require(name)`

`require("name")` loads `name.lua` from your plugin folder; dots are folders, so
`require("lib.format")` loads `lib/format.lua`. Like standard Lua, each module runs once
and later calls return the same value. Module names may only contain letters, digits,
`_` and `-` (plus the dots), and modules can't be loaded from outside the plugin folder.

## Limits

| What | Limit | What happens |
|---|---|---|
| Running time | 250 ms each time the app calls into the plugin: loading the script, all handlers of one event together, the timers due at one moment together, all `shutdown` handlers together | The running callback is stopped with an error; handlers or timers still waiting are skipped (timers run on the next round). |
| Memory per plugin | 32 MiB | The allocation fails with an "out of memory" error. |
| Errors in a row | 10 | The plugin is switched off until it is reloaded. Any successful callback resets the count. |
| Event handlers | 256 | `mp.on` raises an error. |
| Timers at once | 256 | `mp.timer.after`/`every` raise an error. |
| Shortest repeating timer | 10 ms | Shorter intervals are raised to 10 ms. |
| Storage file | 1 MiB, values nested at most 64 levels deep | `mp.storage.set` raises an error and nothing is saved. |
| Notifications | 10 at once, then 2 per second | Extra notifications are dropped (with a warning in the log). |
| Player commands | 50 at once, then 20 per second | Extra commands are dropped (with a warning in the log). |
| Log messages (`mp.log`, `print`) | 100 at once, then 20 per second | Extra messages are dropped (with a warning in the log). |
| Errors reported to the app | 20 at once, then 5 per second | Extra errors are only logged. They still count towards "errors in a row". |
| Notification title / body | 200 / 2000 characters | The text is shortened. |
| Error message | 1000 characters | The message is shortened (the log keeps more). |
| Log message | 4000 characters | The message is shortened. |
| Web requests | 4 waiting at once; 10 started at once, then 1 per second | `mp.http.get` raises an error. |
| Web request time and size | 20 seconds, 4 MiB | The callback gets `nil` and an error message. |
| JSON text | 4 MiB | `mp.json.decode` raises an error. |
| Lyrics | 1 MiB | `done` raises an error, and the plugin counts as having found none. |

Plugins with a lot to do should split work into small steps with `mp.timer.after(0, ...)`
instead of doing everything in one callback.

The time limit covers the standard library and the `mp` functions too: the string
pattern functions (`find`, `match`, `gmatch`, `gsub`) and `table.concat`, `insert`,
`remove`, `move` and `sort` are stopped part-way when time runs out, and `mp` functions
refuse to run once it has. So even a pattern that would backtrack for hours can't freeze
the plugin thread. As a last resort, the app never waits more than three seconds for
plugins when quitting.

## The sandbox

Plugins get Lua's pure standard libraries, and reach outside the app only through
[`mp.http`](#web-requests-mphttp) and [`mp.discord`](#discord-mpdiscord), with the
user's permission:

| Available | |
|---|---|
| Basic functions | `assert`, `error`, `ipairs`, `pairs`, `next`, `pcall`, `xpcall`, `select`, `tonumber`, `tostring`, `type`, `rawequal`, `rawget`, `rawset`, `rawlen`, `getmetatable`, `setmetatable`, `collectgarbage`, `load` (text only), `print` (logs) |
| Libraries | `string`, `table`, `math`, `utf8`, `coroutine` |
| `os` | only `os.time`, `os.clock`, `os.date` and `os.difftime` |

Not available: `io`, `debug`, `package`, `dofile`, `loadfile`, `string.dump`, `warn`, and
the rest of `os` (`os.execute`, `os.getenv`, `os.remove`, `os.exit`, ...). Also:

- `load` only accepts Lua source text, never precompiled bytecode.
- `setmetatable` refuses `__gc` finalizers, because they would run plugin code outside
  any time limit.
- `pcall` and `xpcall` can't catch the time limit: a callback that runs out of time is
  always stopped.
- Errors raised by `mp` functions are plain strings such as
  `main.lua:12: unknown repeat mode 'sometimes' (use "off", "all" or "one")`, so `pcall`
  works as you'd expect.

## Errors and debugging

When a plugin fails to load or a callback raises an error, the plugin manager shows a
short message with the file and line, for example:

```
error in 'track_changed' handler: main.lua:7: attempt to index a nil value (local 'track')
```

The full error with a Lua stack traceback goes to the log. Run the app from a terminal
with `RUST_LOG=plugin=debug` to see every plugin's log output and errors, or
`RUST_LOG=plugin::hello=debug` for a single plugin.

Tips:

- Event arguments can be `nil` (`track_changed` when the queue ends): check before use.
- Keep handlers short. Anything slow belongs in small timer steps.
- Save state in the `shutdown` handler or as it changes; plugins are reloaded from
  scratch when switched off and on again.

## Included plugins

Two plugins come with the app. Like any other they are off until switched on, and they
ask for their permissions first. Their source is in
[`crates/plugins/bundled/`](../crates/plugins/bundled/), laid out like a plugin folder.

### Online Lyrics (`online-lyrics`)

Finds lyrics on [LRCLIB](https://lrclib.net), a free and open lyrics database, for songs
that have none of their own. It only searches when you open a song's lyrics, and only
sends the song's title and artist; the album and length stay on the computer and pick
the best of the results, preferring timed lyrics. Songs LRCLIB has nothing for are
remembered for a month, so they aren't searched again every time.

**Prefer Online Lyrics** has it replace songs' own lyrics too: **Over Untimed Lyrics**
swaps untimed lyrics for timed ones from LRCLIB, and **Always** uses LRCLIB's wherever it
has them, for consistency. Either way, timed lyrics win over untimed ones, so a song's
own timed lyrics are never swapped for untimed ones. **Lyrics Without Timing** turns
plain (untimed) lyrics from LRCLIB off.

Lyrics it finds are kept in the app's cache, or as `.lrc` files beside the songs with
**Save Next to Songs** on the Plugins page (never over a song's own `.lrc` file).

### Discord Status (`discord-status`)

Shows the playing song on your Discord profile, as "Listening to Penguin Player", with
the app's icon. Its settings choose what is shown: the title, artist and album, the
progress bar, whether paused songs stay, and what member lists show after "Listening to".

## Example plugins

Two complete plugins live in [`examples/plugins/`](../examples/plugins/). Copy a folder
into your plugins folder and switch it on.

### now-playing-toast

Shows a notification with the title, artist, album and quality of every new track, such
as "Miles Davis — Kind of Blue · 24-bit / 96 kHz FLAC". It is a single `track_changed`
handler and a good starting point for your own plugin.

### sleep-timer

Pauses playback after a number of minutes, gently fading the volume out first. It shows
how to combine events, timers and storage:

- `settings.lua` is a module (loaded with `require("settings")`) that reads `minutes` and
  `fade_seconds` from `mp.storage`, writing the defaults (30 minutes, 10 seconds) the
  first time. To change them, edit `plugin-data/sleep-timer.json` in the app's data
  folder and reload plugins.
- When playback starts, `mp.timer.after` starts the countdown. Pausing or stopping
  cancels it with `mp.timer.cancel`.
- When the countdown ends, `mp.timer.every` lowers the volume in ten steps, then the
  plugin pauses and restores the original volume.
- Its `shutdown` handler puts the volume back if the plugin is switched off, or the app
  quits, mid-fade.
