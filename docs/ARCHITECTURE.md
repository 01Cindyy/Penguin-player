# Architecture

Penguin Player is an offline, native music player written in Rust with a GTK4 +
libadwaita interface. Linux is the first target. Nothing here should assume Linux
outside clearly isolated, `cfg`-gated modules, so Windows and macOS builds stay
possible.

## Workspace

```
crates/
  core/      mp-core         Data model, settings, play queue, search, LRC lyrics. No GUI, no audio.
  audio/     mp-audio        Playback engine: Symphonia decoding → ring buffer → cpal output. Gapless.
                             Also the visualiser's audio tap (Scope) and analysis (Spectrum).
  library/   mp-library      Folder scanning (lofty tags), SQLite storage, playlists, folder watching.
                             Finding a song's lyrics (its tags or an .lrc file beside it).
  plugins/   mp-plugins      Sandboxed Lua plugin host (one thread, one Lua state per plugin), the
                             plugins shipped with the app, and the web and Discord access the user
                             may allow plugins.
  theme/     mp-theme        theme.json (+ optional style.css) → GTK CSS. Built-in and pywal themes.
  app/       penguin-player  GTK4/libadwaita front end. Glues everything together.
docs/                        Architecture, theme and plugin authoring guides, screenshots.
examples/                    Example user plugins and theme.
install.sh                   Installs the program, its launcher, icon, AppStream info and licences
                             (POSIX sh): system-wide, with root only for copying, or with --user
                             and no root at all. Also the installer of the .tar.gz release bundle.
packaging/                   Release packages (.deb, .rpm, AppImage, .tar.gz) built in a Debian 13
                             container, the third-party licence notice, and the AUR PKGBUILD.
```

Dependency direction is strictly downwards: `app` → {`audio`, `library`, `plugins`, `theme`} → `core`.
The backend crates never depend on each other or on GTK, so each can be tested and reused on its own.

## Threads and data flow

| Thread | Owner | Talks to the UI via |
|---|---|---|
| GTK main thread | `app` | — |
| Decoder thread + cpal callback | `mp-audio` | `async_channel::Receiver<EngineEvent>` |
| Library worker (one, long-lived) | `app` (`services::library`) using `mp-library` | `async_channel` |
| Folder watcher | `mp-library::LibraryWatcher` | queues jobs on the library worker |
| Cover decoding | `app` (`services::covers`) on GIO's blocking pool | `gio::spawn_blocking` futures |
| Plugin host | `mp-plugins` | `async_channel::Receiver<HostEvent>` |
| Plugin web requests (one each) | `mp-plugins` (`http`, ureq) | the plugin host's inbox |
| Discord connection (once a plugin shows something) | `mp-plugins` (`discord`) | — (fed by the plugin host) |
| Media controls (MPRIS) | `souvlaki` | callback → `async_channel` |

The UI consumes every channel with `glib::spawn_future_local`, so all UI state is only
ever touched on the main thread and there is no locking in UI code. The main thread
never does slow I/O: loading and scanning happen on the library worker, covers are
decoded on GIO's blocking pool, and only quick edits (playlists) use the main thread's
own database connection (`AppContext::edit_library`).

The visualiser is the one exception to channels: while one is shown and music plays, the
output callback copies a mono mix of what it plays into `mp_audio::Scope`, a ring of
atomics, and each visualiser reads the newest audio from it on each frame (corrected for
the device's latency) and analyses it with `mp_audio::Spectrum`. Visualisers hold a
`ScopeWatch` while they need audio; nothing is copied while none do.

### Playback and gapless handoff

`app::player::Player` (a GObject) owns the `AudioEngine` and a `mp_core::PlayQueue`.

1. `Player::play_tracks(ids, start)` fills the queue and calls `engine.load(current)`.
2. On `EngineEvent::TrackStarted { from_queue, .. }`: if `from_queue`, the queue moves to
   that track. The player then publishes it as the current track and calls `sync_next`,
   which hands the engine the next playable track with `engine.set_next` so it can
   decode it without a gap. `PlayQueue::following_positions` says what plays next; the
   player passes over tracks that failed or are no longer in the library.
3. Any queue change (shuffle, repeat, play next, reorder, remove) and every library
   refresh calls `sync_next` again.
4. `EngineEvent::EndOfQueue` means nothing was queued in time (gapless off, or a very
   short track): the player moves to the next playable track (`move_to_next_playable`)
   and `load`s it, or stops at the end of the queue.
5. Every `PlayerCommand` (UI buttons, keyboard shortcuts, MPRIS, plugins) goes through
   one `Player::execute` method, so behaviour is identical whichever source issues it.
6. The player broadcasts `mp_core::PlayerEvent`s to the plugin host and media controls.

### Library

- The library worker owns its own `Library` connection and runs jobs in order (load,
  scan, changed paths, watch, clear), so scans never overlap.
- Startup: the worker sends `tracks()` from the database, then runs an incremental
  `scan()` (unchanged files are skipped by size + mtime) and sends the refreshed list
  if anything changed. The first scan of a new library runs while the UI is usable.
- The worker turns tracks into a `model::LibrarySnapshot`: the tracks in the order
  `Library::tracks()` gives them (`mp_core::collection::library_order`, which is also
  the Songs page's default order), the albums and the artists derived from them with
  `mp_core::collection`, format counts and the normalised search text of every track
  and album. The main thread only swaps it into the `LibraryStore` (`gio::ListStore`s
  of GObject wrappers), reusing the objects of unchanged items so views keep their
  state.
- Views put GTK's filter and sort models on top of the store's lists, and the store
  splices only what a refresh changed (`model::sync_list_store`).
- Choosing another music folder cancels a scan of the previous one
  (`Library::scan_cancellable`).
- The watcher reports changed paths in debounced batches → `scan_paths()` on the worker.
  With inotify it watches each visible folder itself, leaving hidden ones (`.git`,
  `.Trash-1000`) out, as scans do.
- A missing or emptied music folder (an unplugged drive) leaves the library untouched
  and shows a banner. While it is missing, every newly mounted drive triggers a scan;
  the first scan that finds the folder again clears the banner and restarts watching.
- Playlists are stored by file path, so they survive rescans and folder changes.
  Entries whose file is gone stay in the playlist and are listed as missing.
  `Library::reorder_playlist` saves a whole new order in one transaction.

### Search

`mp_core::search` normalises text (case and accents) and parses a query
into words. `app::ui::pages::search::results` matches it against each object's
prepared search text (a substring scan with no allocation per item), ranks matches
into a few relevance buckets, and only re-searches the previous matches while a word
is being typed further.

## App crate

### Window layout

```
ToolbarView ─ content: ToastOverlay ─ Stack ─ "onboarding": first-run folder picker
            │                              └ "library": OverlaySplitView (queue panel at the end)
            │                                    └ NavigationSplitView: sidebar | content pane
            └ bottom bar: PlayerBar                content pane: header (back, search),
                                                   folder banner, pages (NavigationView)
```

Below 1040 sp the queue panel floats over the content; below 720 sp the sidebar
collapses and the player bar turns compact. Pages switch to their narrow layout
(`mp-narrow`, stacked headers, slimmer gutters) below 560 sp (`pages::NARROW_PAGE_WIDTH`).

While the Now Playing page is the visible page the window hides the player bar (the
page has its own controls) and the search field, lets the page reach up behind the
header, and shows the sidebar as icons only; leaving restores the sidebar's own size.

With lyrics on, Now Playing shows them in the right half of pages at least 860 sp wide
(an `OverlaySplitView` whose sidebar is half the page) and in place of the art on
narrower ones. Each place has its own `LyricsView`. The page reads the lyrics once per
song on a blocking thread, the song's own with `mp_library::read_lyrics` and those
plugins found before with `read_found_lyrics`, and hands both views the result; each
view follows the player's position by itself. Plugins find lyrics
(`mp.lyrics.provide`) for songs without any, or also for songs whose own lyrics they
may replace; `LyricsSearch` says which songs, all plugins together, and `OwnLyrics` what
a song has. Lyrics found before replace the song's own while plugins would look it up,
and untimed lyrics never replace timed ones. If plugins look the song up and found
nothing before, the page waits a second (so skipping songs doesn't search), asks through
`PluginService::find_lyrics`, shows the song's own lyrics or a spinner meanwhile, and
saves what is found with `mp_library::save_found_lyrics`: in the cache, or as an `.lrc`
file beside the song with `save_found_lyrics` on.

Plugins ask for `permissions` (sites for `mp.http`, `discord` for `mp.discord`) in
`plugin.json`; the Plugins page asks the user before switching such a plugin on and
saves what was allowed in `Settings::plugin_permissions`. The host gives each plugin
what it asks for *and* was allowed. A plugin that asks for more than it was allowed,
such as after an update, is switched off at start. Settings that plugins declare are
saved in `Settings::plugin_settings`, without the defaults.

### Module map

```
crates/app/
  build.rs                  Compiles data/icons/*.svg into a GResource (pure Rust, gvdb).
  data/
    style.css               Base styles and the defaults of most --mp-* variables.
    css/browse.css          Cards, grids, album and artist pages.
    css/collections.css     Playlists, the queue panel, drag and drop.
    css/discover.css        Search, formats, preferences.
    icons/                  Bundled symbolic icons (see its README for licence).
  src/
    main.rs, lib.rs         Logging and start-up.
    application.rs          AdwApplication: stylesheet, actions, accelerators, startup.
    context.rs              AppContext: shared handles (paths, settings, store, player, ...).
    display.rs              DisplayOptions: settings several widgets follow (visualiser,
                            quality details), as properties to bind to.
    screenshot.rs           Debug builds: render the window to a PNG (see below).
    signals.rs              Quits cleanly on SIGTERM, SIGINT and SIGHUP (Unix only).
    util.rs                 Text helpers: counts, lengths, display names.
    player/                 Player GObject: engine + queue + command handling.
    model/                  GObject wrappers (TrackObject, AlbumObject, ArtistObject,
                            PlaylistObject) and LibraryStore / LibrarySnapshot.
    services/
      library.rs            The library worker: load, scan, watch, clear.
      covers.rs             Cover art at display size: LRU in memory, PNG thumbnails on disk.
      theme.rs              ThemeManager: applies themes and the appearance options,
                            live reload (user themes, pywal's colors.json), colour scheme.
      blur.rs               Blur behind the translucent window (Wayland, cfg-gated).
      plugins.rs            PluginService: runs the Lua host, forwards player events,
                            keeps permissions and plugin settings, asks for lyrics.
      media_controls.rs     MPRIS / Now Playing (cfg-gated, isolated).
    ui/
      window.rs             The main window (layout above) and its breakpoints.
      navigation.rs         Route (names a page) and Navigator (builds and shows it).
      sidebar.rs, player_bar.rs, queue_panel.rs, folder_banner.rs, onboarding.rs
      preferences/          The dialog: General, Appearance, Plugins pages (plugin
                            settings pages and permission prompts in plugins.rs).
      dialogs.rs            Shared dialogs: clear library, choose folder, playlists.
      context_menu.rs       Right-click, long press and Menu key, the same everywhere.
      track_actions.rs      Play next, add to queue or playlist, go to album or artist,
                            show in folder.
      shortcuts.rs          Playback keys that don't steal keys from text fields.
      icons.rs              One constant per bundled icon.
      mod.rs                Small shared helpers (empty states, breakpoints, keep_alive).
      pages/                One module per page: songs, albums, album, artists, artist,
                            formats (+ cards, stats), format, now_playing, playlist,
                            search (+ overview, results, full_lists).
      widgets/
        page_header.rs      Compact and hero page headers (title, eyebrow, actions).
        section_header.rs   Section titles with a "Show All" action.
        cover.rs            CoverImage and its sizes; the shared placeholder tile.
        format_badge.rs     FLAC / Hi-Res / lossless badges.
        song_heading.rs     The cover + title + artist start of every song row.
        visualizer.rs       The spectrum bars above the seek bar.
        resize_handle.rs    The draggable edges of the sidebar, queue and player bar.
        balanced_row.rs     The player bar's three columns, controls always centred.
        playback/           Transport buttons, seek bar and volume, shared by the player
                            bar and Now Playing; each follows the player by itself.
        cover_backdrop.rs   Blurred album art behind Now Playing, fading between songs.
        lyrics_view.rs      A song's lyrics; timed ones follow it and seek when clicked.
        track_list/         The virtualised, sortable track table used by most pages.
        browse/             Album and artist cards, CardGrid, CardShelf, DetailPage,
                            GradualStore, the browse.* actions.
        collections/        Playlist mosaic, missing entries, the queue's lists,
                            reorder logic for drag and drop.
        discover/           Search's top result and song list, the composition bar,
                            figures, format colours (codec.rs), theme previews.
```

### Adding a page

1. Add a module under `ui/pages/` with
   `pub fn build(ctx: &Rc<AppContext>, …) -> adw::NavigationPage`. Use
   `pages::page_with_header` for a page whose list scrolls under a fixed header, or
   `widgets::browse::DetailPage` for a page that scrolls as a whole.
2. Add a `Route` variant and build it in `Route::build_page`.
3. Keep the page's Rust state in a struct owned by the page (`ui::keep_alive`) and
   reach it from signal handlers through `Weak` references. Connect to long-lived
   objects (store, player) with `ui::disconnect_on_destroy`.

### Performance notes

The app stays responsive with libraries of 50,000 songs and more. What keeps it so:

- Large lists (`TrackList`, `CardGrid`) are virtualised: they are the direct child of
  a `ScrolledWindow`, so GTK only creates widgets near the visible rows. GTK keeps
  about 200 rows of a list view and about 30 rows of `max-columns` cards of a grid
  view, so `CardGrid` sets `max-columns` from its width rather than to a large fixed
  number.
- Lists inside a scrolling page create every row, so they are kept short
  (`CollapsibleItems`) and filled a chunk per frame (`GradualStore`).
- Walking a `gtk::FilterListModel` item by item is slow on large libraries. To count
  or total what a filter lets through, walk the store with the same predicate (see
  the Songs and Format pages).
- Covers are decoded off the main thread at the size they are shown, kept in a 64 MB
  LRU and cached on disk as thumbnails.

Measured with a synthetic library of 50,000 tagged files in 5,000 albums (release
build, files on tmpfs, 1024×768 window):

| | |
|---|---|
| First scan of a new library | about 1.0 s, in the background |
| Start to first frame | about 0.22 s |
| Start to the full library shown (warm start) | about 0.58 s |
| Memory (RSS) after browsing every page | about 240 MB (110 MB without a library) |
| First search | about 60 ms, including building the search page |
| Later searches | 3–9 ms |
| Switching to Albums or Artists | about 70 ms |
| Switching to Songs | about 140 ms |
| Opening an album or artist | about 25 ms |

## Theming contract

Stylesheets are layered (later wins):

1. libadwaita's own stylesheet.
2. The app's structural styles at `STYLE_PROVIDER_PRIORITY_APPLICATION`:
   `data/style.css` followed by `data/css/browse.css`, `collections.css` and
   `discover.css`, loaded as one stylesheet. They lay out the app's widgets and give
   every `--mp-*` variable a default (most in `style.css`, the card and codec ones next
   to their rules).
3. The active theme's CSS from `mp_theme::Theme::to_css_for_scheme(dark)` at
   `STYLE_PROVIDER_PRIORITY_APPLICATION + 1`, regenerated by `services::theme` whenever
   `adw::StyleManager:dark` changes. It sets libadwaita variables
   (`--accent-bg-color`, `--window-bg-color`, ...) and `--mp-*` variables, then appends
   the theme's own `style.css`. (`Theme::to_css()` holds both schemes under media
   queries instead, which needs GTK 4.20.)

4. The user's appearance options from `mp_theme::Theme::appearance_css(Appearance)` at
   `STYLE_PROVIDER_PRIORITY_APPLICATION + 2`: the corner style (Square and Slight set
   every radius variable and libadwaita's own widget radii; Round adds nothing) and
   transparency (translucent `--mp-window-bg-color`, `--mp-panel-bg-color` and
   `--mp-player-bar-surface`, with the content views made transparent), and defined
   edges (lines in `--mp-edge-color` on the sidebar, queue panel and player bar, and
   shadows on the bottom bar and the page's `mp-edge-shade` strips). The theme's
   `effects` opacities are only hints for this layer. Blur behind the window uses the
   standard `ext-background-effect-v1` protocol: with a GTK that understands CSS
   `backdrop-filter` (4.24), this layer sets it on the window and GTK talks to the
   compositor; otherwise `services::blur` uses the protocol (or KWin's older blur
   protocol) itself. A surface may only have one background-effect object, so the
   app must never create its own when GTK does.

A theme's `scheme` (`any`/`light`/`dark`) decides the `adw::StyleManager` colour scheme;
with `any` the user's preference from settings applies.

Colours in the app's stylesheets always come from variables (libadwaita's or the
`--mp-*` ones below), so every theme works in light and dark without changes there.

### App CSS variables

| Variable | Meaning | Default |
|---|---|---|
| `--mp-panel-radius` | Cards, panels, list containers | `12px` |
| `--mp-cover-radius` | Album art (page headers use 1.5×) | `8px` |
| `--mp-button-radius` | Transport, play and page menu buttons | `999px` |
| `--mp-spacing` | Base spacing unit; page gutters are 2.5× (1.5× when narrow) | `12px` |
| `--mp-window-bg-color` | Main window background (may be translucent) | `var(--window-bg-color)` |
| `--mp-panel-bg-color` | Sidebar / queue panel background | `var(--sidebar-bg-color)` |
| `--mp-player-bar-bg-color` | Now-playing bar background | `var(--headerbar-bg-color)` |
| `--mp-player-bar-fg-color` | Now-playing bar text | `var(--window-fg-color)` |
| `--mp-cover-shadow` | `box-shadow` under covers and artist images | soft drop shadow |
| `--mp-cover-hover-shadow` | `box-shadow` under a card's art while hovered | deeper drop shadow |
| `--mp-hires-badge-color` | Hi-Res badge and meter | `var(--warning-color)` |
| `--mp-lossless-badge-color` | Lossless badge and meter | `var(--accent-color)` |
| `--mp-codec-color` | A format's identity colour, set by each `.mp-codec-<id>` class | libadwaita palette colours |

### Stable CSS classes

Themes may target these; renaming one is a breaking change for user themes.
docs/themes.md lists them for theme authors.

| Area | Classes |
|---|---|
| Window | `mp-main-window`, `mp-sidebar` (+ `mp-icons-only`), `mp-resize-handle` (+ `mp-resize-width`, `mp-resize-height`, `mp-dragging`), `mp-edge-shade` (+ `mp-start`, `mp-end`), `mp-folder-banner`, `mp-onboarding`, `mp-search-entry` |
| Pages | `mp-songs-page`, `mp-albums-page`, `mp-album-page`, `mp-artists-page`, `mp-artist-page`, `mp-formats-page`, `mp-format-page`, `mp-playlist-page`, `mp-search-page` (on each page); `mp-narrow` on page parts in their narrow layout |
| Page headers | `mp-page-header`, `mp-page-eyebrow`, `mp-page-title`, `mp-page-subtitle`, `mp-page-action` (Play, Shuffle), `mp-page-menu-button` |
| Sections and controls | `mp-section-header`, `mp-section-title`, `mp-show-all`, `mp-chip` (filter chips), `mp-link` (text that opens a page) |
| Now Playing | `mp-now-playing-page`, `cover-backdrop` (CSS node), `mp-now-playing-scrim`, `mp-now-playing-column`, `mp-cover-hero`, `mp-now-byline`, `mp-now-album`, `mp-seek-row`, `mp-now-playing-split`, `mp-now-playing-lyrics` |
| Lyrics | `mp-lyrics`, `mp-lyrics-synced` (on the list of timed lyrics), `mp-lyrics-line`, `mp-lyrics-current`, `mp-lyrics-pause`, `mp-lyrics-break` |
| Player bar | `mp-player-bar` (+ `mp-compact`), `mp-now-title`, `mp-now-artist`, `mp-now-quality`, `mp-now-quality-row`, `mp-now-quality-details`, `mp-play-button`, `mp-transport-button`, `mp-toggle-button`, `mp-seek-bar`, `mp-time-label`, `mp-volume` |
| Covers | `mp-cover` with `mp-cover-row` (song rows), `mp-cover-small` (player bar, queue) or `mp-cover-large` (page headers); `mp-cover-placeholder` while there is no art; `mp-cover-mosaic` (playlist artwork); `mp-artist-avatar` (round artist images) |
| Cards | `mp-card-grid` (the grid), `mp-album-card`, `mp-artist-card`, `mp-card-art`, `mp-card-play`, `mp-album-title`, `mp-album-artist`, `mp-artist-name`, `mp-artist-stats` |
| Track lists | `mp-track-list`, `mp-track-table`, `mp-track-title`, `mp-track-artist`, `mp-track-album`, `mp-track-number`, `mp-track-duration`, `mp-track-quality`, `mp-disc-header`, `mp-playing` (the current track's row) |
| Badges | `mp-format-badge` (+ `mp-lossless`, `mp-hires`), `mp-quality-badge` (album page) |
| Search | `mp-search-overview`, `mp-top-result`, `mp-top-result-title`, `mp-top-result-play`, `mp-kind-pill`, `mp-song-rows`, `mp-song-row` |
| Formats | `mp-format-overview`, `mp-composition-bar`, `mp-composition-segment`, `mp-quality-tier`, `mp-meter` (+ `mp-hires`, `mp-lossless`, `mp-lossy`), `mp-format-card`, `mp-codec-tile`, `mp-codec-dot`, `mp-codec-<id>` (`flac`, `alac`, `wav`, `aiff`, `mp3`, `aac`, `opus`, `vorbis`, `unknown`) |
| Queue | `mp-queue-panel`, `mp-now-playing-card`, `mp-queue-list`, `mp-queue-row`, `mp-queue-remove`, `mp-queue-clear` |
| Playlists | `mp-missing-entries`, `mp-missing-row`, `mp-drop-before`, `mp-drop-after` (drop markers), `mp-drag-icon` |

Other `mp-*` classes are layout details and may change.

### Icons

Every icon the app names is bundled in `crates/app/data/icons/` as
`mp-<name>-symbolic.svg`, compiled into a GResource by `build.rs` and registered at
startup (`ui::icons::register`). The `mp-` names keep a system icon theme from
replacing them; GTK's own icons (window controls, the search entry) follow the
platform theme. To add one, copy a symbolic SVG there, add a constant to
`ui/icons.rs` and a row to the folder's README; a unit test checks that every
`-symbolic` name in the source is bundled.

## Files on disk

Resolved by `mp_core::AppPaths` (XDG on Linux, `~/Library` on macOS, `%APPDATA%` on
Windows; everything under `<dir>` with `PENGUIN_PLAYER_HOME=<dir>`):

| Path | Linux default | Contents |
|---|---|---|
| `<config>/settings.json` | `~/.config/penguin-player/` | `mp_core::Settings` |
| `<config>/themes/<id>/theme.json` | | User themes |
| `<pywal cache>/colors.json` | `~/.cache/wal/` | Read only: the Pywal theme's palette (`mp_theme::pywal_dir`) |
| `<config>/plugins/<id>/plugin.json` | | User plugins |
| `<data>/library.db` | `~/.local/share/penguin-player/` | SQLite library |
| `<data>/plugin-data/` | | Plugin storage |
| `<cache>/covers/`, `<cache>/thumbnails/` | `~/.cache/penguin-player/` | Extracted and downscaled cover art |
| `<cache>/lyrics/` | | Lyrics that plugins found, by a hash of the song's path |

## Conventions

- `cargo fmt` and `cargo clippy --workspace --all-targets` must be clean.
- A module with submodules lives in `<name>/mod.rs` (clippy's `self_named_module_files`
  checks it).
- Files from folders users fill (themes, plugins) are read with `mp_core::files`: regular
  files only, size-limited, never outside their folder.
- Library crates return `thiserror` error enums; the app logs with `log` and tells the
  user through toasts. Never panic on user data; `expect` only for real invariants,
  with a message explaining why it can't fail.
- Comments explain *why* or describe a non-obvious block; doc comments on public items
  whose purpose isn't obvious from the name. No comment noise.
- Keep platform specifics behind one module with `cfg` (e.g. media controls); use
  `AppPaths`, GIO and GTK APIs for paths, opening folders and URIs.
- UI text uses title case for titles and buttons and sentence case for descriptions,
  as in the GNOME Human Interface Guidelines.
- A builder's `css_classes([...])` replaces the classes a widget gives itself, such as
  `adw::ButtonRow`'s `button`. Add classes to those widgets with `add_css_class`.
- Empty states say what is missing and where it will come from ("No Albums" /
  "Albums in your music folder will appear here."), and follow the scan state through
  `LibraryStatus`.

## Development environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG=debug` | Verbose logging (`env_logger`). |
| `PENGUIN_PLAYER_HOME=<dir>` | Keep config/data/cache under `<dir>` (portable mode; safe testing). |
| `PENGUIN_PLAYER_NULL_AUDIO=1` | Decode in real time but discard audio (no sound device). |
| `PENGUIN_PLAYER_SCREENSHOT=<file.png>` | Debug builds: render the window to a PNG once loaded, then quit. |
| `PENGUIN_PLAYER_SCREENSHOT_PAGE=<page>` | Page to open first: `songs`, `albums`, `artists`, `formats`, `queue`, `settings[:appearance\|plugins]`, `search:<query>`, `album:<title>`, `artist:<name>`, `playlist:<name>`, `format:<id>`. |
| `PENGUIN_PLAYER_SCREENSHOT_SCHEME=light\|dark` | Force the colour scheme. |
| `PENGUIN_PLAYER_SCREENSHOT_PLAY=<n>` | Play the library from track `n` first (only with `PENGUIN_PLAYER_NULL_AUDIO=1`). |
| `PENGUIN_PLAYER_SCREENSHOT_DELAY=<seconds>` | Wait this long after opening the page (default 1). |
| `PENGUIN_PLAYER_SCREENSHOT_SCROLL=<0…1>` | Scroll the page this far down first. |

The screenshot tool works headless with GTK's Broadway backend
(`gtk4-broadwayd :N &`, then `GDK_BACKEND=broadway BROADWAY_DISPLAY=:N`), which caps
windows at 1024×768. The window size comes from `settings.json`.
