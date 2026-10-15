#!/bin/sh
# Builds the release packages into dist/: a .deb, an .rpm, an AppImage and a .tar.gz
# bundle for x86_64, with THIRD-PARTY-LICENSES.txt and SHA256SUMS. It also refreshes
# THIRD-PARTY-LICENSES.txt in the source folder, which install.sh and the AUR
# package ship.
#
# Everything happens in a rootless Podman container with Debian 13 (see Containerfile),
# so the packages run on Debian 13, Ubuntu 25.04, Fedora 42 and anything newer,
# whichever distribution builds them. The source folder is mounted read-only, and the
# build itself has no network: crates are downloaded in a step of their own first.
#
# Needs podman, and no administrator rights.

set -eu
LC_ALL=C
export LC_ALL

APP_ID=io.github._01cindyy.Penguin_player
IMAGE=localhost/penguin-player-packaging

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'build.sh: %s\n' "$*" >&2
    exit 1
}

[ $# -eq 0 ] || fail "takes no arguments; it builds the version in Cargo.toml"
[ "$(id -u)" != 0 ] || fail "run it as yourself: podman works without root"
command -v podman >/dev/null 2>&1 || fail "podman isn't installed (on Arch: sudo pacman -S podman)"

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)
[ -n "$version" ] || fail "can't find the version in Cargo.toml"
newest=$(sed -n 's/.*<release version="\([^"]*\)".*/\1/p' \
    "$root/crates/app/data/$APP_ID.metainfo.xml" | head -n 1)
[ "$newest" = "$version" ] ||
    fail "the newest release in the metainfo is $newest, but Cargo.toml says $version:" \
        "add a <release> for $version to crates/app/data/$APP_ID.metainfo.xml"

work=$root/target/packaging
dist=$root/dist
mkdir -p "$work/cargo-home" "$work/target" "$dist"

say "Preparing the build environment (the first time takes about ten minutes)..."
podman build --pull=missing --tag "$IMAGE" --file "$root/packaging/Containerfile" "$root/packaging"

# Runs a command in the build environment, with the source read-only and the build's
# own folders writable.
run() {
    podman run --rm --init \
        --volume "$root:/src:ro" \
        --volume "$work:/work" \
        --volume "$dist:/dist" \
        --env CARGO_HOME=/work/cargo-home \
        --env CARGO_TARGET_DIR=/work/target \
        --workdir /src \
        "$@"
}

say "Downloading crates..."
run "$IMAGE" cargo fetch --locked
say "Building Penguin Player $version and its packages..."
run --network=none "$IMAGE" /src/packaging/make-packages.sh "$version"

if cmp -s "$dist/THIRD-PARTY-LICENSES.txt" "$root/THIRD-PARTY-LICENSES.txt"; then
    say "THIRD-PARTY-LICENSES.txt is up to date."
else
    cp -- "$dist/THIRD-PARTY-LICENSES.txt" "$root/THIRD-PARTY-LICENSES.txt"
    say "Updated THIRD-PARTY-LICENSES.txt: commit it with the release."
fi
say "Packages in $dist:"
(cd -- "$dist" && ls -l)
