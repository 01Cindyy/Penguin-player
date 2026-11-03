#!/bin/sh
# Makes the Windows download: a folder with penguin-player.exe and everything it needs
# from MSYS2 (GTK, libadwaita and the libraries, data and licences that come with
# them), zipped as dist/penguin-player-<version>-windows-x86_64.zip.
#
# Run it in MSYS2's UCRT64 shell, with the packages from packaging/README.md:
#
#     packaging/windows/make-bundle.sh
#
# The folder works from anywhere: GTK finds its files relative to the bin folder its
# DLLs are in.

set -eu
LC_ALL=C
export LC_ALL

PROGRAM=penguin-player
PREFIX=/ucrt64

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'make-bundle.sh: %s\n' "$*" >&2
    exit 1
}

[ $# -eq 0 ] || fail "takes no arguments; it builds the version in Cargo.toml"
[ "${MSYSTEM:-}" = UCRT64 ] || fail "run it in MSYS2's UCRT64 shell"
for tool in cargo ldd pacman pkg-config zip; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is missing (see packaging/README.md)"
done

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)
[ -n "$version" ] || fail "can't find the version in Cargo.toml"
name=$PROGRAM-$version-windows-x86_64
target=${CARGO_TARGET_DIR:-$root/target}
bundle=$target/windows/$name
dist=$root/dist

say "== Program"
(cd -- "$root" && cargo build --release --locked -p "$PROGRAM")

say "== Bundle"
rm -rf -- "$bundle" "$dist/$name.zip"
mkdir -p "$bundle/bin" "$bundle/share/glib-2.0/schemas" "$bundle/share/licenses/$PROGRAM" "$dist"
cp -- "$target/release/$PROGRAM.exe" "$bundle/bin/"
cp -- "$root/LICENSE" "$root/THIRD-PARTY-LICENSES.txt" "$bundle/share/licenses/$PROGRAM/"

# Image loaders, for covers. Their list names each loader by its path, which is made
# relative to the bundle so it works wherever the bundle is.
pixbuf=lib/gdk-pixbuf-2.0/$(pkg-config --variable=gdk_pixbuf_binary_version gdk-pixbuf-2.0)
mkdir -p "$bundle/$pixbuf/loaders"
cp -- "$PREFIX/$pixbuf"/loaders/*.dll "$bundle/$pixbuf/loaders/"
awk -v dir="$pixbuf/loaders/" '
    /^".*\.dll"$/ {
        name = $0
        gsub(/^"|"$/, "", name)
        gsub(/.*[\\\/]/, "", name)
        print "\"" dir name "\""
        next
    }
    { print }
' "$PREFIX/$pixbuf/loaders.cache" >"$bundle/$pixbuf/loaders.cache"

# GTK's settings schemas, and the helper GLib uses to keep to one copy of the app.
cp -- "$PREFIX/share/glib-2.0/schemas/gschemas.compiled" "$bundle/share/glib-2.0/schemas/"
cp -- "$PREFIX/bin/gdbus.exe" "$bundle/bin/"

# Every DLL from MSYS2 that the program, the image loaders and gdbus load.
dlls=$(ldd "$bundle/bin/$PROGRAM.exe" "$bundle/bin/gdbus.exe" "$bundle/$pixbuf"/loaders/*.dll |
    awk -v prefix="$PREFIX/bin/" 'index($3, prefix) == 1 { print $3 }' | sort -u)
[ -n "$dlls" ] || fail "found no DLLs for the program; is GTK installed?"
for dll in $dlls; do
    cp -- "$dll" "$bundle/bin/"
done

say "== Licences"
# The licences of the MSYS2 packages the DLLs and data come from, and where to get
# their source code.
packages=$(for file in $dlls "$PREFIX/bin/gdbus.exe" "$PREFIX/$pixbuf"/loaders/*.dll; do
    pacman -Qqo "$file"
done | sort -u)
for package in $packages; do
    licences=$PREFIX/share/licenses/${package#mingw-w64-ucrt-x86_64-}
    if [ -d "$licences" ]; then
        cp -R -- "$licences" "$bundle/share/licenses/"
    fi
done
{
    printf 'Libraries in Penguin Player for Windows\n\n'
    printf 'The DLLs in bin, and the files in lib and share/glib-2.0, are unchanged copies\n'
    printf 'from these MSYS2 packages (https://www.msys2.org). Their licences are in\n'
    printf 'share/licenses, and their source code is at\n'
    printf 'https://github.com/msys2/MINGW-packages and https://repo.msys2.org/mingw/sources/\n\n'
    # shellcheck disable=SC2086 # one argument per package
    pacman -Q $packages | sed 's/^/    /'
} >"$bundle/share/licenses/LIBRARIES.txt"

say "== Zip"
(cd -- "$target/windows" && zip -qr9 "$dist/$name.zip" "$name")
say "Made $dist/$name.zip"
