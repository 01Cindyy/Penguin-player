# Bundled icons

Every icon the app shows is bundled here, so it looks the same with any icon theme and
on systems without one. `build.rs` compiles the SVGs into a GResource and
`ui::icons::register` adds it to the icon theme at startup. The names are the app's
own (`mp-…-symbolic`), so a system theme can never replace them, and the
`-symbolic` suffix lets GTK recolour them like any symbolic icon.

To add an icon, copy a symbolic SVG here as `mp-<name>-symbolic.svg`, add a constant
for it to `src/ui/icons.rs` and a row to the table below.

## Licence

The icons are unmodified copies, renamed, of icons from the
[Adwaita icon theme](https://gitlab.gnome.org/GNOME/adwaita-icon-theme) 51.0 by the
GNOME Project (<https://www.gnome.org>). The Adwaita icon theme's licence notice:

> This work is licenced under the terms of either the GNU LGPL v3 or Creative Commons
> Attribution-Share Alike 3.0 United States License.
>
> To view a copy of the CC-BY-SA licence, visit
> <http://creativecommons.org/licenses/by-sa/3.0/> or send a letter to Creative
> Commons, 171 Second Street, Suite 300, San Francisco, California 94105, USA.
>
> When attributing the artwork, using "GNOME Project" is enough.
> Please link to <http://www.gnome.org> where available.

The LGPL v3 is at <https://www.gnu.org/licenses/lgpl-3.0.html>.

## Sources

| Bundled as | Adwaita icon |
|---|---|
| `mp-add-symbolic` | `actions/list-add-symbolic` |
| `mp-album-symbolic` | `devices/media-optical-cd-audio-symbolic` |
| `mp-appearance-symbolic` | `categories/applications-graphics-symbolic` |
| `mp-artist-symbolic` | `status/avatar-default-symbolic` |
| `mp-back-symbolic` | `actions/go-previous-symbolic` |
| `mp-check-symbolic` | `actions/object-select-symbolic` |
| `mp-disc-symbolic` | `devices/media-optical-symbolic` |
| `mp-expand-symbolic` | `ui/pan-down-symbolic` |
| `mp-formats-symbolic` | `devices/audio-card-symbolic` |
| `mp-general-symbolic` | `categories/preferences-system-symbolic` |
| `mp-headphones-symbolic` | `devices/audio-headphones-symbolic` |
| `mp-guide-symbolic` | `legacy/accessories-dictionary-symbolic` |
| `mp-in-order-symbolic` | `status/media-playlist-consecutive-symbolic` |
| `mp-lyrics-symbolic` | `actions/media-view-subtitles-symbolic` |
| `mp-menu-symbolic` | `actions/open-menu-symbolic` |
| `mp-more-symbolic` | `actions/view-more-symbolic` |
| `mp-music-folder-symbolic` | `places/folder-music-symbolic` |
| `mp-open-folder-symbolic` | `status/folder-open-symbolic` |
| `mp-pause-symbolic` | `actions/media-playback-pause-symbolic` |
| `mp-play-symbolic` | `actions/media-playback-start-symbolic` |
| `mp-plugins-symbolic` | `categories/applications-utilities-symbolic` |
| `mp-preferences-symbolic` | `legacy/emblem-system-symbolic` |
| `mp-queue-symbolic` | `actions/view-list-symbolic` |
| `mp-refresh-symbolic` | `actions/view-refresh-symbolic` |
| `mp-remove-symbolic` | `actions/list-remove-symbolic` |
| `mp-repeat-one-symbolic` | `status/media-playlist-repeat-song-symbolic` |
| `mp-repeat-symbolic` | `status/media-playlist-repeat-symbolic` |
| `mp-search-symbolic` | `actions/system-search-symbolic` |
| `mp-shuffle-symbolic` | `status/media-playlist-shuffle-symbolic` |
| `mp-skip-backward-symbolic` | `actions/media-skip-backward-symbolic` |
| `mp-skip-forward-symbolic` | `actions/media-skip-forward-symbolic` |
| `mp-song-symbolic` | `mimetypes/audio-x-generic-symbolic` |
| `mp-sort-descending-symbolic` | `actions/view-sort-descending-symbolic` |
| `mp-trash-symbolic` | `places/user-trash-symbolic` |
| `mp-unavailable-symbolic` | `actions/action-unavailable-symbolic` |
| `mp-volume-high-symbolic` | `status/audio-volume-high-symbolic` |
| `mp-volume-low-symbolic` | `status/audio-volume-low-symbolic` |
| `mp-volume-medium-symbolic` | `status/audio-volume-medium-symbolic` |
| `mp-volume-muted-symbolic` | `status/audio-volume-muted-symbolic` |
| `mp-warning-symbolic` | `legacy/emblem-important-symbolic` |

## App icon

`io.github._01cindyy.Penguin_player.svg` is the app's own icon (named after `mp_core::APP_ID`),
made for this project and licensed like the rest of it. It is bundled so the window
and About dialog have it without installation; `install.sh` installs it for the
desktop launcher, along with `app-icon.png` as the 32×32 size.

It is pixel art, drawn at 32×32 (`app-icon.png`, the source). The SVG has one square
per pixel, merged into one shape per colour, with `shape-rendering="crispEdges"`, so
it stays sharp at any size instead of blurring the way a scaled PNG does: exact at
whole multiples of 32, and with slightly uneven pixels in between. After changing the
PNG, convert it again the same way.
