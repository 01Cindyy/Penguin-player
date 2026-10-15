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

It also refreshes `THIRD-PARTY-LICENSES.txt` in the source folder, which `install.sh`
and the AUR package install.

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
3. Run `packaging/build.sh`, and commit `THIRD-PARTY-LICENSES.txt` if it changed.
4. Tag the commit `v<version>`, push it, and make a GitHub release from the tag with
   the files in `dist/`.
5. Update the AUR package from `packaging/aur/`: set `pkgver` (and `pkgrel=1`), then

   ```sh
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   makepkg --cleanbuild
   ```

   and copy `PKGBUILD` and `.SRCINFO` into the AUR repository.
