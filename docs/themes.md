# Making themes

A theme changes how the player looks: its colours, how round the corners are, how much
breathing room there is, shadows, the font, and (with a little CSS) the look of
individual parts of the window.

Three appearance options in Preferences → Appearance work on top of every theme:
**Corners** (Square, Slight or Round, where Round keeps the theme's own radii),
**Defined Edges** (lines in the theme's text colour between the sidebar, queue, player
bar and page, with a soft shadow from the theme's shade colour) and **Transparency** (a
translucent window, with blur behind it where the desktop allows).

The app ships with these themes:

| Theme | Id | Look |
|---|---|---|
| **Adwaita** (default) | `adwaita` | The native GNOME look. Follows your system's light/dark preference and accent colour, just a little rounder and airier. |
| **Midnight** | `midnight` | A deeper dark mode: near-black surfaces for OLED screens and late nights, with your system accent colour. Dark only. |
| **Soft Rice** | `rice` | Muted lavender with teal touches on deep charcoal (or soft lilac in light mode), with a floating player bar. |
| **Frost** | `frost` | Calm blue-grey with frosted blue accents, gentle contrast, light and dark. |
| **Catppuccin** | `catppuccin` | Latte in light mode, Mocha in dark. |
| **Dracula** | `dracula` | The classic Dracula palette. Dark only. |
| **Everforest** | `everforest` | Soft forest greens, light and dark. |
| **Gruvbox** | `gruvbox` | Retro, warm and earthy, light and dark. |
| **Kanagawa** | `kanagawa` | Lotus in light mode, Wave in dark. |
| **Nord** | `nord` | Snow Storm in light mode, Polar Night in dark. |
| **One Dark** | `one-dark` | One Light in light mode, One Dark in dark. |
| **Rosé Pine** | `rose-pine` | Dawn in light mode, Main in dark. |
| **Solarized** | `solarized` | Solarized light and dark. |
| **Tokyo Night** | `tokyo-night` | Day in light mode, Night in dark. |

The palettes follow each project's published colours, adjusted where needed so every
text and badge meets WCAG AA contrast.

### Pywal

If you use [pywal](https://github.com/eylles/pywal16) (or the original pywal), a
**Pywal** theme appears in the list as well. It is made from the palette pywal picked
from your wallpaper and changes along with it: run `wal -i <wallpaper>` and the player
recolours itself straight away, without a restart or any setup.

- The background and text colours are pywal's own; the sidebar, cards and menus are
  slightly lighter (or, for a light palette from `wal -l`, slightly darker) steps of the
  background. A dark palette gives a dark theme, a light palette a light one.
- The accent is the most colourful of `color1` to `color6`.
- Every colour is nudged towards white or black where needed to meet the same WCAG AA
  contrast as the built-in themes, so a murky wallpaper never makes text unreadable.

The app reads `colors.json` from pywal's cache folder, found the way pywal finds it:
`$PYWAL_CACHE_DIR`, else `$XDG_CACHE_HOME/wal`, else `~/.cache/wal`. The theme is only
listed while that file exists. A user theme with the id `pywal` replaces it, like any
built-in.

You can add your own without writing any code: a theme is a folder with a `theme.json`
file and, optionally, a `style.css`.

- [Your first theme](#your-first-theme)
- [Theme folder layout](#theme-folder-layout)
- [theme.json reference](#themejson-reference)
- [Light and dark](#light-and-dark)
- [Custom CSS with style.css](#custom-css-with-stylecss)
- [Live reload](#live-reload)
- [Tips](#tips)
- [Worked example: Sunset](#worked-example-sunset)

## Your first theme

1. Open the themes folder in the app's config folder:

   | System | Themes folder |
   |---|---|
   | Linux | `~/.config/penguin-player/themes/` (or `$XDG_CONFIG_HOME/penguin-player/themes/`) |
   | Portable mode (`PENGUIN_PLAYER_HOME=<dir>`) | `<dir>/config/themes/` |

2. Create a folder for the theme. **The folder name is the theme's id**, for example
   `mint`.

3. Add `mint/theme.json`:

   ```json
   {
     "name": "Mint",
     "author": "You",
     "light": { "accent": "#2f7a5f", "accent_fg": "#ffffff" },
     "dark": { "accent": "#7fd1b0", "accent_fg": "#10241c" },
     "shape": { "panel_radius": 16, "cover_radius": 12 }
   }
   ```

4. Pick **Mint** in the app's appearance settings.

Everything you leave out keeps its default, so a theme can be as small as one colour.

## Theme folder layout

```
themes/
  mint/                the folder name is the theme id
    theme.json         required
    style.css          optional extra CSS, picked up automatically
```

Folders without a `theme.json` and hidden folders (starting with `.`) are ignored. A theme
whose `theme.json` can't be read is skipped and the reason is logged; it never stops the
app from starting.

A user theme with the same id as a built-in one (such as `adwaita`, `rice` or `gruvbox`)
replaces it.
That is the easiest way to tweak a built-in theme: copy its files from
[`crates/theme/themes/`](../crates/theme/themes/) and edit them.

## theme.json reference

All fields are optional.

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Name shown in the theme picker. Defaults to the folder name. |
| `author` | string | Who made it. |
| `description` | string | One sentence about the theme. |
| `version` | string | Free form, for your own bookkeeping. |
| `scheme` | `"any"`, `"light"` or `"dark"` | Which colour scheme the theme supports. Default `"any"`. See [Light and dark](#light-and-dark). |
| `colors` | [palette](#palette) | Colours used in both light and dark mode. |
| `light` | [palette](#palette) | Colours for light mode; override `colors`. |
| `dark` | [palette](#palette) | Colours for dark mode; override `colors`. |
| `shape` | [shape](#shape) | Corner radii and spacing. |
| `effects` | [effects](#effects) | Translucency and shadows. |
| `font_family` | string | Font for the whole window, e.g. `"Inter, Cantarell, sans-serif"`. |
| `stylesheet` | string | Extra CSS file, relative to the theme folder. Defaults to `style.css` if that file exists. |

### Palette

Every key is optional; anything you don't set keeps libadwaita's colour (which follows
the system accent colour and light/dark mode). Each `_bg` colour has a matching `_fg`
text colour, so set them in pairs to keep text readable.

| Key | Colours | Sets |
|---|---|---|
| `accent` | Accent: selected items, the play button, sliders | `--accent-bg-color` |
| `accent_fg` | Text and icons on the accent colour | `--accent-fg-color` |
| `destructive` | Destructive buttons and errors | `--destructive-bg-color`, `--error-bg-color` |
| `success` | Success messages | `--success-bg-color` |
| `warning` | Warnings | `--warning-bg-color` |
| `window_bg`, `window_fg` | The window | `--window-bg-color` (and the unfocused header bar), `--window-fg-color` |
| `view_bg`, `view_fg` | Lists and content areas | `--view-bg-color`, `--view-fg-color` |
| `headerbar_bg`, `headerbar_fg` | The header bar | `--headerbar-bg-color`, `--headerbar-fg-color` |
| `sidebar_bg`, `sidebar_fg` | The navigation sidebar and queue panel | `--sidebar-bg-color`, `--secondary-sidebar-bg-color` (and their unfocused versions), `--sidebar-fg-color`, `--secondary-sidebar-fg-color` |
| `card_bg`, `card_fg` | Cards and boxed lists | `--card-bg-color`, `--card-fg-color` |
| `popover_bg`, `popover_fg` | Menus and popovers | `--popover-bg-color`, `--popover-fg-color` |
| `dialog_bg`, `dialog_fg` | Dialogs | `--dialog-bg-color`, `--dialog-fg-color` |
| `player_bar_bg`, `player_bar_fg` | The now-playing bar | `--mp-player-bar-bg-color`, `--mp-player-bar-fg-color` |
| `border` | Borders and separators | `--border-color` |
| `hires_badge` | The "Hi-Res" quality badge | `--mp-hires-badge-color` |

libadwaita works out the accent *text* colour (`--accent-color`, used for links and
highlighted labels) from `accent` by itself, adjusting it to stay readable in light and
dark mode.

**Colour formats.** `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(…)`, `rgba(…)`, `hsl(…)`
and `hsla(…)`, with either commas or spaces: `"#7259b0"`, `"#00000020"`,
`"rgb(43 40 54 / 12%)"`, `"hsl(260deg 35% 52%)"`. Anything else, such as colour names
(`"red"`) or `var(…)`, is ignored with a warning in the log. Use `style.css` when you need
more.

### Shape

Whole pixels, all optional.

| Key | Sets | Default | Allowed |
|---|---|---|---|
| `window_radius` | Rounding of the window corners (`--window-radius`); windows are square while maximised, tiled or fullscreen | 15 | 0–48 |
| `panel_radius` | Cards, panels and list containers (`--mp-panel-radius`) | 12 | 0–999 |
| `cover_radius` | Album art (`--mp-cover-radius`) | 8 | 0–999 |
| `button_radius` | Play, previous and next buttons (`--mp-button-radius`); `999` makes them round | 999 | 0–999 |
| `spacing` | The base spacing unit (`--mp-spacing`) | 12 | 0–48 |

Larger values are lowered to the maximum. Negative or fractional numbers make the whole
`theme.json` invalid.

### Effects

| Key | Meaning | Allowed |
|---|---|---|
| `window_opacity` | How opaque the window background is while the user has transparency on. `0.9` lets 10% of the wallpaper through. Default 0.84. | 0.5–1.0 |
| `panel_opacity` | The same for the sidebar, queue panel and now-playing bar. Default 0.76. | 0.5–1.0 |
| `cover_shadow` | Strength of the shadow under album art (`--mp-cover-shadow`); `0` turns it off. | 0.0–1.0 |

Values outside the range are clamped, so text always stays readable. Transparency is
the user's choice (Preferences → Appearance), not the theme's: the opacities above only
tune how translucent the theme looks once it is switched on. Blur behind the window
works on KDE Plasma; on Hyprland it follows your Hyprland settings.

### Font

`font_family` is a comma-separated list, tried in order; quotes are optional. Generic
families (`serif`, `sans-serif`, `monospace`, `cursive`, `fantasy`, `system-ui`,
`emoji`) are understood too. End the list with one of them as a fallback.

## Light and dark

The `scheme` field decides which modes a theme supports:

- `"any"` (default): the theme follows the light/dark choice in the app's settings,
  which in turn can follow your system.
- `"light"` or `"dark"`: the app always uses that mode while the theme is active.

Colours are combined like this: `colors` applies in both modes, then `light` or `dark`
overrides individual keys for that mode. A typical theme puts shared colours (for
example `destructive`) in `colors` and everything else in `light` and `dark`:

```json
{
  "colors": { "destructive": "#b8455e" },
  "light": { "window_bg": "#f5f2f9", "window_fg": "#2b2836" },
  "dark": { "window_bg": "#16151b", "window_fg": "#e7e2ef" }
}
```

A theme with `"scheme": "dark"` only needs `colors` (or `dark`).

## Custom CSS with style.css

For anything `theme.json` doesn't cover, add a `style.css` to the theme folder. It is
added after the generated colours and wins over the app's own styling. Stylesheets are
layered like this (later wins):

1. libadwaita's stylesheet,
2. the app's structural stylesheet, which gives every `--mp-*` variable a default,
3. your theme: the CSS generated from `theme.json`, then your `style.css`.

GTK CSS is close to web CSS but not identical: it styles colours, backgrounds, borders,
radii, shadows, margins and padding, fonts and transitions, but not layout (`display`,
`flex`, `width`, ...). See the
[GTK CSS overview](https://docs.gtk.org/gtk4/css-overview.html) and
[properties](https://docs.gtk.org/gtk4/css-properties.html). These all work in the
GTK version the app targets: `var(--name)`, `color-mix(in srgb, …)`, `alpha(color, 0.5)`
and `@media (prefers-color-scheme: dark) { … }`.

A theme can't load other files: no images through `url()` and no `@import`. If
`style.css` contains either (even inside a string, though not in a comment), the whole
stylesheet is left out and the app tells you so; the colours from `theme.json` still
apply. Gradients (`linear-gradient(…)` and friends) work for backgrounds instead.

### Variables

Use these in `style.css` to stay consistent with the theme and to adapt to light and
dark mode automatically.

From libadwaita:

| Variable | Meaning |
|---|---|
| `--accent-bg-color`, `--accent-fg-color`, `--accent-color` | Accent background, text on it, accent-coloured text |
| `--window-bg-color`, `--window-fg-color` | Window |
| `--view-bg-color`, `--view-fg-color` | Lists and content |
| `--headerbar-bg-color`, `--headerbar-fg-color` | Header bar |
| `--sidebar-bg-color`, `--sidebar-fg-color` | Sidebar |
| `--card-bg-color`, `--card-fg-color` | Cards |
| `--popover-bg-color`, `--dialog-bg-color` | Popovers, dialogs |
| `--border-color` | Borders |
| `--window-radius` | Window corners |

The full list is in the
[libadwaita documentation](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/css-variables.html).

The app's own:

| Variable | Meaning |
|---|---|
| `--mp-panel-radius` | Cards, panels, list containers |
| `--mp-cover-radius` | Album art; page headers use one and a half times this |
| `--mp-button-radius` | Play, previous and next, and the round "More Options" buttons |
| `--mp-small-radius` | Small rounded things: row highlights, badges, links, previews (9px) |
| `--mp-pill-radius` | Pill shapes: search field, filter chips, small buttons, meters (999px) |
| `--mp-spacing` | Base spacing unit; page margins are 2.5 units (1.5 on narrow windows) |
| `--mp-window-bg-color` | Main window background (may be translucent) |
| `--mp-panel-bg-color` | Sidebar and queue panel background |
| `--mp-player-bar-bg-color`, `--mp-player-bar-fg-color` | Now-playing bar colours |
| `--mp-player-bar-surface` | What the bar is painted with: its colour, made translucent by the transparency option |
| `--mp-cover-shadow` | Shadow under album art and artist images (a `box-shadow` value) |
| `--mp-cover-hover-shadow` | Shadow under a card's art while the pointer is on it (only settable in CSS) |
| `--mp-hires-badge-color` | Hi-Res badge and meter |
| `--mp-lossless-badge-color` | Lossless badge and meter (only settable in CSS) |
| `--mp-codec-color` | A format's colour; see [Format colours](#format-colours) |

### Format colours

Every format has a colour of its own, used by the Formats page (the composition bar,
its legend and the cards) and by the tile at the top of a format's page. A class per
format sets it as `--mp-codec-color`: `.mp-codec-flac`, `.mp-codec-alac`,
`.mp-codec-wav`, `.mp-codec-aiff`, `.mp-codec-mp3`, `.mp-codec-aac`, `.mp-codec-opus`,
`.mp-codec-vorbis` and `.mp-codec-unknown`. The defaults come from libadwaita's palette
and are chosen to stay apart from their neighbours in light and dark mode. To change
one:

```css
.mp-codec-flac {
  --mp-codec-color: #3d7fd9;
}
```

### CSS classes

These class names are stable: they won't change without notice, so your theme keeps
working across updates. Other `mp-` classes you may find in the app are layout
details that can change.

| Class | What |
|---|---|
| `.mp-main-window` | The main window |
| `.mp-sidebar` | Navigation sidebar (with `.mp-icons-only` while dragged narrow) |
| `.mp-edge-shade` | Strips along the page's edges (`.mp-start` beside the sidebar, `.mp-end` beside the queue) where Defined Edges draws the panels' shadows |
| `.mp-resize-handle` | The edges dragged to resize the sidebar and the queue panel (`.mp-resize-width`) and the player bar (`.mp-resize-height`); `.mp-dragging` while dragged |
| `.mp-search-entry` | The search field |
| `.mp-folder-banner` | The warning shown while the music folder is unavailable |
| `.mp-onboarding` | First-run folder picker |
| `.mp-songs-page`, `.mp-albums-page`, `.mp-album-page`, `.mp-artists-page`, `.mp-artist-page`, `.mp-formats-page`, `.mp-format-page`, `.mp-playlist-page`, `.mp-search-page` | Each page, to style one page only |
| `.mp-narrow` | Added to page headers, lists and grids while the window is narrow |
| `.mp-page-header`, `.mp-page-eyebrow`, `.mp-page-title`, `.mp-page-subtitle` | Page headers: the small label above the title, the title, the line under it |
| `.mp-page-action`, `.mp-page-menu-button` | The Play and Shuffle buttons of a page, and its round "More Options" button |
| `.mp-section-header`, `.mp-section-title`, `.mp-show-all` | Section headings and their "Show All" buttons |
| `.mp-chip` | Filter chips, such as Lossless or FLAC on the Songs page |
| `.mp-link` | Text that opens a page, such as an artist's name |
| `.mp-player-bar` | Now-playing bar (with `.mp-compact` on narrow windows); inside it `.mp-now-title`, `.mp-now-artist`, `.mp-now-quality` (the format badge) and, with full quality details on, `.mp-now-quality-row` holding the badge and `.mp-now-quality-details` |
| `.mp-play-button`, `.mp-transport-button`, `.mp-toggle-button` | Play/pause, previous/next, shuffle/repeat |
| `.mp-seek-bar`, `.mp-time-label`, `.mp-volume` | Seek slider, time labels, volume control |
| `.mp-visualizer` | The visualiser above the seek bar: its bars are drawn in its `color` (the accent by default), as tall as the player bar's top edge is dragged |
| `.mp-cover` | Any album art; with `.mp-cover-row` in song rows, `.mp-cover-small` in the player bar and queue, `.mp-cover-large` in page headers |
| `.mp-cover-placeholder` | Album art that is missing or still loading |
| `.mp-cover-mosaic` | A playlist's artwork |
| `.mp-artist-avatar` | Round artist image (an artist without art shows their initials on the placeholder tile) |
| `.mp-card-grid` | Grids of album or artist cards |
| `.mp-album-card`, `.mp-artist-card` | Album and artist cards; inside them `.mp-card-art`, `.mp-card-play` (the play button shown on hover), `.mp-album-title`, `.mp-album-artist`, `.mp-artist-name`, `.mp-artist-stats` |
| `.mp-track-list`, `.mp-track-table` | Track lists; inside them `.mp-track-title`, `.mp-track-artist`, `.mp-track-album`, `.mp-track-number`, `.mp-track-duration`, `.mp-track-quality` |
| `.mp-playing` | The row of the track that is playing |
| `.mp-disc-header` | "Disc 1", "Disc 2" on album pages |
| `.mp-format-badge` (with `.mp-lossless`, `.mp-hires`) | Format and quality badges; `.mp-quality-badge` on album pages |
| `.mp-search-overview` | The search page's "All" view |
| `.mp-top-result`, `.mp-top-result-title`, `.mp-top-result-play`, `.mp-kind-pill` | The search page's Top Result card, its title, play button and "Album"/"Song" pill |
| `.mp-song-rows`, `.mp-song-row` | Short song lists, such as on the search page |
| `.mp-format-overview`, `.mp-composition-bar`, `.mp-composition-segment` | The Formats page's library overview and its bar |
| `.mp-quality-tier`, `.mp-meter` (with `.mp-hires`, `.mp-lossless`, `.mp-lossy`) | Quality tiers and their meters |
| `.mp-format-card` | Cards on the Formats page |
| `.mp-codec-tile`, `.mp-codec-dot` | A format's tile on its page, and its dot in legends and cards |
| `.mp-now-playing-page` | The Now Playing page: `cover-backdrop` (the blurred art), `.mp-now-playing-scrim` (the tint over it that keeps text readable), `.mp-now-playing-column` (art and controls), `.mp-cover-hero` (the art), `.mp-now-title`, `.mp-now-byline` (artist and album, with `.mp-now-album`), `.mp-now-quality-row` |
| `.mp-lyrics` | Lyrics on Now Playing: a list of `.mp-lyrics-line` labels, with `.mp-lyrics-synced` on the list when they are timed and `.mp-lyrics-current` on the row being sung. Pauses in timed lyrics are `.mp-lyrics-pause`, gaps between verses in plain ones `.mp-lyrics-break`. Beside the art they sit in `.mp-now-playing-lyrics` |
| `.mp-seek-row` | A seek bar with its time labels (and the visualiser above it), in the player bar and on Now Playing |
| `.mp-queue-panel` | Play queue side panel; inside it `.mp-now-playing-card`, `.mp-queue-list`, `.mp-queue-row`, `.mp-queue-remove`, `.mp-queue-clear` |
| `.mp-missing-entries`, `.mp-missing-row` | A playlist's missing files |
| `.mp-drop-before`, `.mp-drop-after`, `.mp-drag-icon` | Where dragged songs will land, and the pill that follows the pointer |

Some ideas:

```css
/* A glowing play button. */
.mp-play-button {
  box-shadow: 0 4px 14px color-mix(in srgb, var(--accent-bg-color) 40%, transparent);
}

/* Square album art everywhere. */
.mp-cover {
  border-radius: 0;
}

/* Your own variable, different in dark mode. */
:root {
  --my-highlight: #c2456b;
}

@media (prefers-color-scheme: dark) {
  :root {
    --my-highlight: #e0798f;
  }
}

.mp-track-list .mp-playing .mp-track-title {
  color: var(--my-highlight);
}
```

`style.css` is limited to 512 KiB, must be inside the theme folder and can't refer to
other files (`url()`, `@import`); otherwise it is left out entirely and the app tells
you why. CSS errors are logged with their line number; GTK skips the broken rule and
keeps the rest.

## Live reload

While one of your themes is active, the app watches its folder. Save `theme.json` or
`style.css` and the change appears straight away, without a restart. The Pywal theme
does the same with pywal's `colors.json`. If an edit leaves
`theme.json` invalid, the app keeps showing the last working version and tells you what's
wrong.

## Tips

- **Readable text.** Aim for a contrast ratio of at least 4.5:1 between each `_fg`
  colour and its background (WCAG AA). Badge text counts too: it sits on a light tint of
  its own colour, so in light mode `hires_badge` needs to be a shade darker than it would
  on the plain background. All built-in themes meet this in both modes.
- **Start small.** An accent colour and a couple of radii already make a theme feel
  different. Add surfaces one pair at a time.
- **Prefer variables.** In `style.css`, `var(--accent-bg-color)` follows the theme and
  the light/dark mode; a fixed colour doesn't.
- **Debugging.** Run the app from a terminal with `RUST_LOG=mp_theme=debug` to see which
  values were ignored and why.

## Worked example: Sunset

[`examples/themes/sunset/`](../examples/themes/sunset/) is a warm peach-and-rose theme
that uses every part of the format. Copy the folder into your themes folder to try it.

**`theme.json`** sets:

- `"scheme": "any"`, so it works in light and dark mode.
- `colors`: `destructive`, `success` and `warning`, shared by both modes.
- `light`: a burnt-orange accent (`#b04e28`) with white text on cream backgrounds
  (`#fbf3ee`), and dark brown text (`#3a2622`) for every surface.
- `dark`: a soft peach accent (`#f2a074`) with dark text on it, on deep plum-brown
  backgrounds (`#1e1517`) with warm off-white text.
- `shape`: rounder windows (16 px) and panels (16 px), 12 px album art, a little more
  spacing (14 px).
- `effects`: a deeper cover shadow (`0.9`).
- `font_family`: `"Inter, Cantarell, sans-serif"`.

**`style.css`** adds what `theme.json` can't express:

```css
/* A rose colour of the theme's own, lighter in dark mode. */
:root {
  --sunset-rose: #c2456b;
}

@media (prefers-color-scheme: dark) {
  :root {
    --sunset-rose: #e0798f;
  }
}

/* A warm glow fading in from the left of the now-playing bar. */
.mp-player-bar {
  background-image: linear-gradient(90deg, color-mix(in srgb, var(--accent-bg-color) 14%, transparent), transparent 65%);
}

/* The quality badge sits on that glow, so in light mode its text is a shade deeper. */
@media (prefers-color-scheme: light) {
  .mp-player-bar {
    --mp-hires-badge-color: #6c4d11;
    --mp-lossless-badge-color: #8e3916;
  }
}

/* A gradient play button, from the accent to rose. */
.mp-play-button {
  background-image: linear-gradient(135deg, var(--accent-bg-color), color-mix(in srgb, var(--accent-bg-color) 60%, var(--sunset-rose)));
  color: var(--accent-fg-color);
}

/* Bolder page titles. */
.mp-page-title {
  font-weight: 800;
  letter-spacing: -0.5px;
}
```

Apart from the badge colours meant for light mode only, everything refers to variables,
so the same stylesheet works in both modes: the accent, and the rose defined above, switch
automatically.
