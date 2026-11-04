# Packaging

How release packages are made. Users install with `install.sh` in the source folder,
or from these packages; see the main README.

## What `build.sh` makes

```sh
packaging/build.sh
```

builds the version in `Cargo.toml` into `dist/`:

| File | For |
|---|---|
| `penguin-player_<version>-1_amd64.deb` | Debian 13, Ubuntu 25.04 and newer |
| `penguin-player-<version>-1.x86_64.rpm` | Fedora 42 and newer |
| `penguin-player-<version>-x86_64.AppImage` | Any distribution with GTK 4.18 and libadwaita 1.7 or newer |
| `penguin-player-<version>-x86_64.tar.gz` | The program with `install.sh`, for any such distribution |
| `THIRD-PARTY-LICENSES.txt`, `SHA256SUMS` | The licences of what the program is built with, and the files' checksums |

It also refreshes `THIRD-PARTY-LICENSES.txt` in the source folder, which `install.sh`,
the AUR package and the Windows installer install. It covers the crates of Linux and
Windows builds alike.

Everything is built in a rootless Podman container with Debian 13 (`Containerfile`):
the oldest distribution with the GTK and libadwaita the app needs, so the program runs
on it and on anything newer, whichever distribution builds it. A program built on a
newer system, such as Arch, needs a newer glibc and would not start on those. The
build environment is pinned: the Rust image by checksum, cargo-about by version and
the AppImage runtime by release and checksum. The source folder is mounted read-only,
and the build has no network once the crates are downloaded.

`build.sh` needs only `podman`, and no administrator rights. The build environment is
made the first time (about ten minutes) and reused after that; `podman system reset`
removes it, and everything else Podman has stored.

`make-packages.sh` runs inside the container. Every package holds what `install.sh`
installs, staged by the `install.sh` of the `.tar.gz`, and the script checks along the
way that:

- the program needs no newer glibc than Debian 13's (2.41);
- it loads with Debian 13's libraries;
- the launcher and the AppStream info are valid;
- the AppImage unpacks to the same program.

The AppImage holds only the app: GTK and libadwaita come from the system it runs on.

`about.toml` lists the licences dependencies may use. A dependency under another
licence stops the build until that licence is checked and added; `about.hbs` lays out
the notice.

## Making a release

1. Set the version in `Cargo.toml` (`[workspace.package]`) and run `cargo update -w`.
2. Add a `<release>` with the version and date at the top of the releases in
   `crates/app/data/io.github._01cindyy.Penguin_player.metainfo.xml`.
3. Run `packaging/build.sh`, and commit `THIRD-PARTY-LICENSES.txt` if it changed. It
   clears the old packages from `dist/`, so run it before the next step.
4. On Windows, run `packaging/windows/make-installer.sh` on that commit (see
   [Windows](#windows-msys2) below). Copy the `.exe` into `dist/` here and, in `dist/`,
   add it to the checksums:

   ```sh
   sha256sum -- *-setup.exe >>SHA256SUMS
   ```

5. Tag the commit `v<version>`, push it, and make a GitHub release from the tag with
   the files in `dist/`.
6. Update the AUR package from `packaging/aur/`: set `pkgver` (and `pkgrel=1`), then

   ```sh
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   makepkg --cleanbuild
   ```

   and copy `PKGBUILD` and `.SRCINFO` into the AUR repository.

## Windows (MSYS2)

Penguin Player is built for Windows (x86_64) in [MSYS2](https://www.msys2.org)'s
UCRT64 environment, which has GTK, libadwaita, Rust and the compilers.

### Setting up, once

1. Install MSYS2, and open **MSYS2 UCRT64** from the Start menu.
2. Run `pacman -Syu`. If it closes the window, open it again and run it once more.
3. Install what the build needs:

   ```sh
   pacman -S --needed git make mingw-w64-ucrt-x86_64-{gcc,pkgconf,cmake,rust,gtk4,libadwaita}
   ```

4. To make the installer, also install [Inno Setup](https://jrsoftware.org/isinfo.php) 7.
   It isn't part of MSYS2, so run this in PowerShell:

   ```powershell
   winget install --id JRSoftware.InnoSetup.7 -e -s winget -i
   ```

### Trying it

In the UCRT64 shell:

```sh
git clone https://github.com/01Cindyy/Penguin-player.git
cd Penguin-player
cargo run
```

`cargo run` keeps a console window open beside the app with its log. A release build
(`cargo run --release`) opens none, like the download.

The app keeps its settings in `%APPDATA%\_01cindyy\penguin-player\config`, its library in
`%APPDATA%\_01cindyy\penguin-player\data`, and covers and lyrics in
`%LOCALAPPDATA%\_01cindyy\penguin-player\cache`.

### The installer

```sh
packaging/windows/make-installer.sh
```

builds the program and makes `dist/penguin-player-<version>-windows-x86_64-setup.exe`
with Inno Setup (`packaging/windows/penguin-player.iss`). What it installs is first
gathered in `target/windows/`: `bin\penguin-player.exe`, the DLLs it loads from MSYS2,
GTK's image loaders and settings schemas, and in `share\licenses` the licences of the
app, its crates and the MSYS2 packages, with where to get their source code
(`LIBRARIES.txt`). That folder runs from wherever it is, without installing it.

The installer asks whether to install for the current user, which needs no administrator
rights, or for everyone. It adds a Start menu shortcut and, if asked, a desktop one, and
lists the app under **Open with** and in Default Apps for the audio formats it plays.
`.mp4` and `.webm` are left out, as those files are usually video. Uninstalling leaves
the app's settings and library in place. It isn't signed, so Windows warns about it
before it runs (see the main README).

`build.rs` gives the program its icon
(`crates/app/data/windows/penguin-player.ico`) and its name, through MSYS2's `windres`.

