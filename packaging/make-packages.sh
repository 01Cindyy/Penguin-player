#!/bin/sh
# Runs inside the packaging container (see build.sh), with the source in /src: builds
# the program and the third-party licence notice, then the packages, into /dist.
#
# Every package holds the files install.sh installs, staged with the install.sh of the
# .tar.gz bundle, so they all install the same thing.

set -eu
LC_ALL=C
export LC_ALL

version=$1
APP_ID=io.github._01cindyy.Penguin_player
PROGRAM=penguin-player
HOMEPAGE=https://github.com/01Cindyy/Penguin-player
MAINTAINER="01Cindyy <$HOMEPAGE>"
# The glibc of Debian 13, the oldest distribution the packages are for.
MAX_GLIBC=2.41
SUMMARY="Your music library, offline and in full quality"
DESCRIPTION="Penguin Player plays the music files you already have. Point it at a
folder to browse, search and play your library, with nothing going online.
It plays FLAC, ALAC, WAV, AIFF, MP3, AAC, Opus and Ogg Vorbis gapless and in
full quality, shows timed lyrics, has themes and sandboxed Lua plugins, and
works with media keys and the desktop's media controls."

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'make-packages.sh: %s\n' "$*" >&2
    exit 1
}

out=/work/out
rm -rf "$out" /work/bundle /work/stage /work/deb /work/rpm /work/appimage
mkdir -p "$out"

say "== Program"
cargo build --frozen --release -p "$PROGRAM"
program=$CARGO_TARGET_DIR/release/$PROGRAM
needed=$(objdump -T "$program" | sed -n 's/.*GLIBC_\([0-9.]*\).*/\1/p' | sort -uV | tail -n 1)
newest=$(printf '%s\n%s\n' "$needed" "$MAX_GLIBC" | sort -V | tail -n 1)
[ "$newest" = "$MAX_GLIBC" ] ||
    fail "the program needs glibc $needed, newer than the $MAX_GLIBC of Debian 13"
say "Needs glibc $needed or newer."

say "== Third-party licence notice"
cargo about generate --frozen --config /src/packaging/about.toml \
    --output-file "$out/THIRD-PARTY-LICENSES.txt" /src/packaging/about.hbs

say "== .tar.gz bundle"
name=$PROGRAM-$version-x86_64
bundle=/work/bundle/$name
mkdir -p "$bundle/data/icons"
cp /src/install.sh "$program" /src/LICENSE /src/README.md "$out/THIRD-PARTY-LICENSES.txt" "$bundle/"
cp "/src/crates/app/data/$APP_ID.desktop" "/src/crates/app/data/$APP_ID.metainfo.xml" "$bundle/data/"
cp "/src/crates/app/data/icons/$APP_ID.svg" /src/crates/app/data/icons/app-icon.png \
    "$bundle/data/icons/"
tar -C /work/bundle --sort=name --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name"

# What every package installs, under /usr.
stage=/work/stage
"$bundle/install.sh" --prefix /usr --destdir "$stage" --no-build
appstreamcli validate --no-net "$stage/usr/share/metainfo/$APP_ID.metainfo.xml"
# Loading every library symbol now proves the program runs with Debian 13's libraries.
LD_BIND_NOW=1 "$stage/usr/bin/$PROGRAM" --help >/dev/null

say "== .deb"
deb=/work/deb
cp -a "$stage" "$deb"
mkdir -p "$deb/DEBIAN" "$deb/usr/share/doc/$PROGRAM" /work/shlibs/debian
{
    cat /src/LICENSE
    printf '\nThe third-party software built into Penguin Player, and its licences, are\n'
    printf 'listed in /usr/share/licenses/%s/THIRD-PARTY-LICENSES.txt.\n' "$PROGRAM"
} >"$deb/usr/share/doc/$PROGRAM/copyright"
# dpkg-shlibdeps names the packages (and versions) the program's libraries come in.
printf 'Source: %s\n\nPackage: %s\nArchitecture: amd64\n' "$PROGRAM" "$PROGRAM" \
    >/work/shlibs/debian/control
# Its versions come from the functions the program calls, but the app's CSS also needs
# GTK 4.18, which no function shows.
depends=$(cd /work/shlibs && dpkg-shlibdeps -O -e"$deb/usr/bin/$PROGRAM" |
    sed -n 's/^shlibs:Depends=//p' |
    sed -e 's/libgtk-4-1 ([^)]*)/libgtk-4-1 (>= 4.18)/' \
        -e 's/libadwaita-1-0 ([^)]*)/libadwaita-1-0 (>= 1.7)/')
for needed in "libgtk-4-1 (>= 4.18)" "libadwaita-1-0 (>= 1.7)"; do
    case $depends in
    *"$needed"*) ;;
    *) fail "dpkg-shlibdeps gave unexpected dependencies: $depends" ;;
    esac
done
{
    printf 'Package: %s\n' "$PROGRAM"
    printf 'Version: %s-1\n' "$version"
    printf 'Architecture: amd64\n'
    printf 'Maintainer: %s\n' "$MAINTAINER"
    printf 'Installed-Size: %s\n' "$(du -sk --exclude=DEBIAN "$deb" | cut -f1)"
    printf 'Depends: %s, hicolor-icon-theme\n' "$depends"
    printf 'Section: sound\nPriority: optional\n'
    printf 'Homepage: %s\n' "$HOMEPAGE"
    printf 'Description: %s\n' "$SUMMARY"
    printf '%s\n' "$DESCRIPTION" | sed 's/^/ /'
} >"$deb/DEBIAN/control"
dpkg-deb --root-owner-group -Zxz --build "$deb" "$out/${PROGRAM}_$version-1_amd64.deb"

say "== .rpm"
rpm=/work/rpm
mkdir -p "$rpm/SPECS"
cat >"$rpm/SPECS/$PROGRAM.spec" <<EOF
Name: $PROGRAM
Version: $version
Release: 1
Summary: $SUMMARY
License: MIT
URL: $HOMEPAGE
Packager: $MAINTAINER
BuildArch: x86_64
Requires: gtk4 >= 4.18
Requires: libadwaita >= 1.7
Requires: hicolor-icon-theme

# The program is built, and stripped, already.
%global debug_package %{nil}
%global __strip /bin/true

%description
$DESCRIPTION

%install
cp -a $stage/. %{buildroot}/

%files
%{_bindir}/$PROGRAM
%{_datadir}/applications/$APP_ID.desktop
%{_datadir}/icons/hicolor/scalable/apps/$APP_ID.svg
%{_datadir}/icons/hicolor/32x32/apps/$APP_ID.png
%{_datadir}/metainfo/$APP_ID.metainfo.xml
%dir %{_datadir}/licenses/$PROGRAM
%license %{_datadir}/licenses/$PROGRAM/LICENSE
%license %{_datadir}/licenses/$PROGRAM/THIRD-PARTY-LICENSES.txt
EOF
rpmbuild -bb --quiet --define "_topdir $rpm" "$rpm/SPECS/$PROGRAM.spec"
mv "$rpm/RPMS/x86_64/$PROGRAM-$version-1.x86_64.rpm" "$out/"

say "== AppImage"
appdir=/work/appimage/AppDir
mkdir -p /work/appimage
cp -a "$stage" "$appdir"
ln -s "usr/bin/$PROGRAM" "$appdir/AppRun"
sed "s|^Exec=.*|Exec=$PROGRAM|" "$stage/usr/share/applications/$APP_ID.desktop" \
    >"$appdir/$APP_ID.desktop"
cp "$stage/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg" "$appdir/"
cp "$stage/usr/share/icons/hicolor/32x32/apps/$APP_ID.png" "$appdir/.DirIcon"
mksquashfs "$appdir" /work/appimage/AppDir.squashfs -root-owned -noappend -comp zstd -quiet
appimage=$out/$name.AppImage
cat /usr/local/share/appimage-runtime-x86_64 /work/appimage/AppDir.squashfs >"$appimage"
chmod 755 "$appimage"
# Unpacking it (which needs no FUSE) proves the runtime finds and reads the files.
(cd /work/appimage && "$appimage" --appimage-extract >/dev/null)
cmp -s "/work/appimage/squashfs-root/usr/bin/$PROGRAM" "$program" ||
    fail "the AppImage doesn't hold the program"

say "== Checksums"
(cd "$out" && sha256sum -- *.deb *.rpm *.AppImage *.tar.gz >SHA256SUMS)
find /dist -maxdepth 1 -type f \( -name "$PROGRAM[-_]*" -o -name SHA256SUMS \
    -o -name THIRD-PARTY-LICENSES.txt \) -delete
cp -- "$out"/* /dist/
