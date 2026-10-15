#!/bin/sh
# Installs Penguin Player as a desktop app: the program, its launcher (.desktop file),
# its icon, its AppStream info and its licences, so it appears in the desktop's app
# launcher and software centre.
#
# It works in the source code, where it builds the program, and in a release bundle
# (the .tar.gz download), which comes with the program built.
#
# By default it installs for everyone, into /usr/local. It builds as the user who ran
# it and asks for administrator rights only to copy the files, by running itself
# again through sudo, doas or run0 for that step alone. With --user it installs into
# the user's home folder and never needs administrator rights.
#
# Only the files listed in --help are created or replaced, each through a temporary
# file renamed into place, and --uninstall removes exactly those. The users' settings,
# libraries and caches are never touched.

set -eu
umask 022
LC_ALL=C
export LC_ALL

APP_ID=io.github._01cindyy.Penguin_player # mp_core::APP_ID
PROGRAM=penguin-player
DEFAULT_PREFIX=/usr/local

usage() {
    cat <<EOF
Usage: ./install.sh [options]

Builds Penguin Player and installs it for everyone on this computer, into
$DEFAULT_PREFIX. Run it as yourself, without sudo: it builds as you and asks for
administrator rights (through sudo, doas or run0) only to copy the files.

Options:
  --user         Install for you alone, into your home folder. Needs no
                 administrator rights.
  --prefix DIR   Install into DIR/bin and DIR/share instead of $DEFAULT_PREFIX.
  --destdir DIR  Put the files under DIR as if it were /, for building packages.
                 DESTDIR=DIR works too. Packages usually run:
                   ./install.sh --prefix /usr --destdir "\$pkgdir" --no-build
  --build-only   Only build the program; install nothing.
  --no-build     Install the program already built in target/release.
  --uninstall    Remove what an install with the same options put in place.
  -h, --help     Show this help.

In a release bundle the program comes built, so there is nothing to build.

What gets installed:
  $DEFAULT_PREFIX/bin/$PROGRAM
  $DEFAULT_PREFIX/share/applications/$APP_ID.desktop
  $DEFAULT_PREFIX/share/icons/hicolor/scalable/apps/$APP_ID.svg
  $DEFAULT_PREFIX/share/icons/hicolor/32x32/apps/$APP_ID.png
  $DEFAULT_PREFIX/share/metainfo/$APP_ID.metainfo.xml
  $DEFAULT_PREFIX/share/licenses/$PROGRAM/LICENSE
  $DEFAULT_PREFIX/share/licenses/$PROGRAM/THIRD-PARTY-LICENSES.txt
With --prefix, DIR/bin and DIR/share take the place of $DEFAULT_PREFIX/bin and
$DEFAULT_PREFIX/share. With --user, ~/.local/bin and ~/.local/share do (or
\$XDG_DATA_HOME, if you set it).
EOF
}

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

# `path` without trailing slashes, rejected unless absolute and free of control
# characters. `what` names it in errors.
clean_path() {
    what=$1
    path=$2
    case $path in
    /*) ;;
    *) fail "$what must be an absolute path, not '$path'" ;;
    esac
    case $path in
    *[[:cntrl:]]*) fail "$what can't contain control characters" ;;
    esac
    while [ "$path" != / ] && [ "${path%/}" != "$path" ]; do
        path=${path%/}
    done
    printf '%s\n' "$path"
}

scope=system
prefix=$DEFAULT_PREFIX
prefix_given=no
destdir=${DESTDIR:-}
action=install
build=yes
while [ $# -gt 0 ]; do
    case $1 in
    --user)
        scope=user
        shift
        ;;
    --prefix)
        [ $# -ge 2 ] || fail "--prefix needs a folder"
        prefix=$2
        prefix_given=yes
        shift 2
        ;;
    --prefix=*)
        prefix=${1#--prefix=}
        prefix_given=yes
        shift
        ;;
    --destdir)
        [ $# -ge 2 ] || fail "--destdir needs a folder"
        destdir=$2
        shift 2
        ;;
    --destdir=*)
        destdir=${1#--destdir=}
        shift
        ;;
    --build-only)
        action=build
        shift
        ;;
    --no-build)
        build=no
        shift
        ;;
    --uninstall)
        action=uninstall
        shift
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *) fail "unknown option '$1' (see --help)" ;;
    esac
done

if [ "$action" = build ] && [ "$build" = no ]; then
    fail "--build-only and --no-build leave nothing to do"
fi
if [ "$scope" = user ] && [ "$prefix_given" = yes ]; then
    fail "--user installs into your home folder, so it can't take --prefix"
fi

if [ "$scope" = system ]; then
    prefix=$(clean_path "--prefix" "$prefix")
    [ "$prefix" = / ] && prefix=
    bindir=$prefix/bin
    datadir=$prefix/share
else
    [ -n "${HOME:-}" ] || fail "HOME isn't set, so there is no home folder to install into"
    home=$(clean_path "HOME" "$HOME")
    bindir=$home/.local/bin
    # The XDG spec says to ignore a relative XDG_DATA_HOME.
    case ${XDG_DATA_HOME:-} in
    /*) datadir=$(clean_path "XDG_DATA_HOME" "$XDG_DATA_HOME") ;;
    *) datadir=$home/.local/share ;;
    esac
fi
if [ -n "$destdir" ]; then
    destdir=$(clean_path "--destdir" "$destdir")
    [ "$destdir" = / ] && destdir=
fi

is_root() {
    [ "$(id -u)" = 0 ]
}

root=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
self=$root/$(basename -- "$0")
target_dir=
if [ -f "$root/Cargo.toml" ] && [ -d "$root/crates/app/data" ]; then
    data=$root/crates/app/data
    target_dir=${CARGO_TARGET_DIR:-target}
    case $target_dir in
    /*) ;;
    *) target_dir=$root/$target_dir ;;
    esac
    built=$target_dir/release/$PROGRAM
elif [ -f "$root/$PROGRAM" ] && [ -d "$root/data" ]; then
    # A release bundle.
    [ "$action" != build ] || fail "this release bundle comes with the program built already"
    data=$root/data
    built=$root/$PROGRAM
    build=no
else
    fail "install.sh must be run from the Penguin Player folder it came with"
fi

if is_root; then
    if [ "$scope" = user ] && [ "$action" != build ]; then
        fail "--user installs into your own home folder: run it as yourself, without sudo."
    fi
    if [ "$action" != uninstall ] && [ "$build" = yes ]; then
        fail "don't run it with sudo: as yourself, './install.sh' builds the app as you and" \
            "asks for administrator rights only to copy it. To copy a program you already" \
            "built, run 'sudo ./install.sh --no-build'."
    fi
fi

program=$bindir/$PROGRAM
launcher=$datadir/applications/$APP_ID.desktop
svg_icon=$datadir/icons/hicolor/scalable/apps/$APP_ID.svg
png_icon=$datadir/icons/hicolor/32x32/apps/$APP_ID.png
metainfo=$datadir/metainfo/$APP_ID.metainfo.xml
licenses=$datadir/licenses/$PROGRAM
licence=$licenses/LICENSE
notice=$licenses/THIRD-PARTY-LICENSES.txt

# The closest folder above `path` (or `path` itself) that exists.
nearest_existing() {
    path=$1
    while [ ! -e "$path" ] && [ ! -L "$path" ]; do
        path=$(dirname -- "$path")
    done
    printf '%s\n' "$path"
}

# Whether every file in "$@" can be written or removed; if not, `blocked` names the
# folder that can't be written to. Fails if a folder is in the way of a file.
can_write() {
    for file in "$@"; do
        [ ! -d "$destdir$file" ] || fail "$destdir$file is a folder; leaving it alone"
        folder=$(nearest_existing "$(dirname -- "$destdir$file")")
        [ -d "$folder" ] || fail "$folder is in the way: it isn't a folder"
        if [ ! -w "$folder" ]; then
            blocked=$folder
            return 1
        fi
    done
}

# Makes sure every file in "$@" can be written or removed before anything changes.
# Where only root may write, a system-wide install runs this script again with
# administrator rights, for copying (or removing) alone; nothing else ever asks.
need_write_access() {
    can_write "$@" && return 0
    # The copy run with administrator rights is marked, so it never asks again.
    if [ "$scope" = system ] && ! is_root && [ -z "${PENGUIN_PLAYER_INSTALL_ESCALATED:-}" ]; then
        run_as_root
    fi
    if [ "$scope" = system ]; then
        fail "can't write to $blocked. Run './install.sh --user' to install for you alone."
    fi
    fail "can't write to $blocked: check its permissions"
}

run_as_root() {
    set -- --prefix "${prefix:-/}" --no-build
    [ -z "$destdir" ] || set -- "$@" --destdir "$destdir"
    [ "$action" != uninstall ] || set -- "$@" --uninstall
    for tool in sudo doas run0; do
        if command -v "$tool" >/dev/null 2>&1; then
            say "Writing to $blocked needs administrator rights; asking through $tool."
            exec "$tool" env PENGUIN_PLAYER_INSTALL_ESCALATED=1 \
                ${target_dir:+"CARGO_TARGET_DIR=$target_dir"} "$self" "$@"
        fi
    done
    fail "writing to $blocked needs administrator rights, and neither sudo, doas nor run0" \
        "is available. Run '$self $*' as root, or './install.sh --user' to install for you alone."
}

# The Exec value that runs `path`, quoted and escaped as the Desktop Entry
# Specification asks when it has characters other than letters, digits and /._+-.
# A % can't be escaped: GLib looks for the program before turning %% back into %.
desktop_exec() {
    case $1 in
    *%*) fail "the launcher can't start a program whose path has a % in it: $1" ;;
    *[!A-Za-z0-9/._+-]*)
        escaped=$(printf '%s' "$1" | sed -e 's/\\/\\\\\\\\/g' -e 's/["`$]/\\\\&/g')
        printf '"%s"\n' "$escaped"
        ;;
    *) printf '%s\n' "$1" ;;
    esac
}

# A temporary file beside `file`, made safely even in folders others can write to.
# Removed on exit unless it was renamed into place.
temporary=
cleanup() {
    if [ -n "$temporary" ]; then
        rm -f -- "$temporary"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
make_temporary() {
    folder=$(dirname -- "$1")
    mkdir -p -- "$folder"
    temporary=$(mktemp "$folder/.$(basename -- "$1").XXXXXX")
}

# Replaces `file` with `temporary`, given `mode`, in one step: nothing ever sees a
# half-written file, and a running copy of the program keeps working.
finish() {
    chmod "$2" "$temporary"
    mv -f -- "$temporary" "$1"
    temporary=
}

# Copies `source` to `file` (under destdir) with `mode`.
put() {
    make_temporary "$destdir$2"
    cp -- "$1" "$temporary"
    finish "$destdir$2" "$3"
}

# Writes the launcher with `exec_line`, which starts the program by its full path so
# it works whether or not the folder it is in is on PATH.
put_launcher() {
    make_temporary "$destdir$launcher"
    while IFS= read -r line || [ -n "$line" ]; do
        case $line in
        Exec=*) printf '%s\n' "$exec_line" ;;
        *) printf '%s\n' "$line" ;;
        esac
    done <"$data/$APP_ID.desktop" >"$temporary"
    finish "$destdir$launcher" 644
}

# Desktops read icons from a cache when the icon folder has one, so a stale one would
# hide the new icon. Packages leave this to the package manager.
refresh_icon_cache() {
    hicolor=$datadir/icons/hicolor
    if [ -n "$destdir" ] || [ ! -f "$hicolor/icon-theme.cache" ]; then
        return 0
    fi
    for tool in gtk4-update-icon-cache gtk-update-icon-cache; do
        if command -v "$tool" >/dev/null 2>&1; then
            "$tool" -q -t -f "$hicolor" || say "Note: couldn't update the icon cache in $hicolor."
            return 0
        fi
    done
    say "Note: $hicolor has an icon cache, which may need updating before the icon shows."
}

build_program() {
    command -v cargo >/dev/null 2>&1 ||
        fail "cargo isn't installed: the README says how to install Rust."
    say "Building Penguin Player (the first build takes a few minutes)..."
    (cd -- "$root" && cargo build --release --locked -p "$PROGRAM")
}

install_files() {
    [ -f "$built" ] && [ -x "$built" ] ||
        fail "there is no program at $built: build it first (leave out --no-build)."
    exec_line="Exec=$(desktop_exec "$program")"
    set -- "$program" "$launcher" "$svg_icon" "$png_icon" "$metainfo" "$licence" "$notice"
    need_write_access "$@"
    put "$built" "$program" 755
    put_launcher
    put "$data/icons/$APP_ID.svg" "$svg_icon" 644
    put "$data/icons/app-icon.png" "$png_icon" 644
    put "$data/$APP_ID.metainfo.xml" "$metainfo" 644
    put "$root/LICENSE" "$licence" 644
    put "$root/THIRD-PARTY-LICENSES.txt" "$notice" 644
    refresh_icon_cache

    say "Installed Penguin Player:"
    for file in "$@"; do
        say "  $destdir$file"
    done
    if command -v desktop-file-validate >/dev/null 2>&1; then
        desktop-file-validate "$destdir$launcher" ||
            say "Note: desktop-file-validate found problems with the launcher (above)."
    fi
    [ -z "$destdir" ] || return 0

    say "It is in the app launcher now. If it doesn't show up, log out and back in."
    # Root's PATH isn't the user's, so this is only checked for --user installs.
    [ "$scope" = user ] || return 0
    case ":${PATH:-}:" in
    *":$bindir:"*)
        found=$(command -v "$PROGRAM" 2>/dev/null || true)
        if [ -n "$found" ] && [ "$found" != "$program" ]; then
            say "Note: '$PROGRAM' in a terminal starts $found, which comes first on your PATH."
        fi
        ;;
    *) say "To start it from a terminal by name too, add $bindir to your PATH." ;;
    esac
}

uninstall_files() {
    set --
    for file in "$program" "$launcher" "$svg_icon" "$png_icon" "$metainfo" "$licence" "$notice"; do
        if [ -e "$destdir$file" ] || [ -L "$destdir$file" ]; then
            set -- "$@" "$file"
        fi
    done
    if [ $# -eq 0 ]; then
        say "Penguin Player isn't installed there; nothing to remove."
        return 0
    fi
    need_write_access "$@"
    for file in "$@"; do
        rm -f -- "$destdir$file"
        say "Removed $destdir$file"
    done
    # The licences have a folder of their own, removed once it is empty.
    rmdir -- "$destdir$licenses" 2>/dev/null || true
    refresh_icon_cache
    say "Settings, libraries and caches are kept, in the penguin-player folders of"
    say "~/.config, ~/.local/share and ~/.cache. Delete those to remove them too."
}

case $action in
build) build_program ;;
install)
    [ -f "$root/THIRD-PARTY-LICENSES.txt" ] ||
        fail "THIRD-PARTY-LICENSES.txt is missing: packaging/build.sh makes it."
    if [ "$build" = yes ]; then
        build_program
    fi
    install_files
    ;;
uninstall) uninstall_files ;;
esac
