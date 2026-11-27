# Packaging and releases

How the install packages are built, what they install, and how a release is cut. Everything here
follows `.github/workflows/release.yml`, `.github/workflows/build.yml` and the files in
`packaging/`. If this page and those files disagree, the files are right.

## The two workflows

| Workflow | Runs on | Makes |
|---|---|---|
| `build.yml` (Build) | every push to `main`, every pull request, and on demand (Actions > Build > Run workflow) | plain release builds of `wiringcraft` and `wiringcraft-app`, one download per system under the run's "Artifacts" |
| `release.yml` (Release) | a pushed tag `v*`, a pull request that changes `packaging/**` or `release.yml`, and on demand (Actions > Release > Run workflow) | the install packages: Windows setup `.exe`, Linux `.deb`, macOS `.dmg` |

The Build artifacts are named `wiringcraft-windows-x64`, `wiringcraft-linux-x64` and
`wiringcraft-macos-arm64`. Each holds both programs, `README.md`, `LICENSE-MIT`,
`LICENSE-APACHE`, `DISCLAIMER.md` and the `library` folder. The Linux and macOS ones hold a
`.tar.gz` inside, because the artifact zip drops the executable bit. They are kept for 30 days.

The Release workflow builds, installs, runs (`wiringcraft --version`) and uninstalls each package
in CI before it uploads it. The packages are uploaded as the artifacts `package-windows-x64`,
`package-linux-x64` and `package-macos-universal`, kept for 30 days.

## Version

The version comes from the first `version = "..."` line in the root `Cargo.toml`
(`[workspace.package]`). On a tag run the tag must be `v` plus that version (for example `v0.1.0`
for `version = "0.1.0"`), or the job fails with "tag ... does not match version ... in
Cargo.toml".

## Windows: setup .exe

Built on `windows-latest` with Inno Setup 6 from `packaging/windows/wiringcraft.iss`. Output:
`wiringcraft-<version>-windows-x64-setup.exe`.

What it installs:

- Per user, no administrator rights (`PrivilegesRequired=lowest`), into
  `%LOCALAPPDATA%\Programs\WiringCraft`.
- `wiringcraft-app.exe`, `wiringcraft.exe`, `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`,
  `DISCLAIMER.md` and the `library` folder.
- A Start menu entry "WiringCraft" and "Uninstall WiringCraft".
- The install folder is added to the user's `Path` (`HKEY_CURRENT_USER\Environment`), so
  `wiringcraft` works in a new terminal. The uninstaller removes the folder and the `Path` entry.
- The setup shows `LICENSE-MIT` as its license page and offers to start the app at the end.

The programs link the C runtime in (`.cargo/config.toml` sets `+crt-static` for
`x86_64-pc-windows-msvc`), so they need no Visual C++ Redistributable. CI checks that neither
program refers to `VCRUNTIME140`. Both programs get the WiringCraft icon from
`packaging/windows/icon_build.rs`, the build script of `wc-cli` and `wc-app`. It embeds
`assets/icon/wiringcraft.ico` when the target is Windows; without a resource compiler the build
still works, only without the icon.

Build it locally on Windows (Rust stable, Inno Setup 6), from Git Bash for the first part:

```sh
cargo build --release -p wc-cli -p wc-app
mkdir -p stage dist
cp target/release/wiringcraft.exe target/release/wiringcraft-app.exe stage/
cp README.md LICENSE-MIT LICENSE-APACHE DISCLAIMER.md stage/
cp -r library stage/
cp assets/icon/wiringcraft.ico stage/
```

Then in PowerShell, with the version from `Cargo.toml`:

```powershell
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" /Q "/DAppVersion=0.1.0" "/DSourceDir=$PWD\stage" "/DOutputDir=$PWD\dist" packaging\windows\wiringcraft.iss
```

`AppVersion` and `SourceDir` are required; `OutputDir` defaults to the current folder. CI
installs Inno Setup with `choco install innosetup` when it is missing.

## Linux: .deb

Built on `ubuntu-22.04`, the oldest supported Ubuntu runner, so the package also runs on newer
distributions. Output: `wiringcraft-<version>-linux-amd64.deb`.

What it installs:

| Path | Contents |
|---|---|
| `/usr/bin/wiringcraft`, `/usr/bin/wiringcraft-app` | the two programs |
| `/usr/share/applications/wiringcraft.desktop` | the desktop entry (`packaging/linux/wiringcraft.desktop`) |
| `/usr/share/icons/hicolor/<n>x<n>/apps/wiringcraft.png` | the icon at 16, 24, 32, 48, 64, 128, 256 and 512 |
| `/usr/share/icons/hicolor/scalable/apps/wiringcraft.svg` | the scalable icon |
| `/usr/share/doc/wiringcraft/` | `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `DISCLAIMER.md` |
| `/usr/share/wiringcraft/library` | the starter library |

Package fields: `Depends: libc6 (>= 2.35), libgcc-s1, git` and
`Recommends: libgl1, libxkbcommon-x11-0, libwayland-client0`. Install it with
`sudo apt install ./wiringcraft-<version>-linux-amd64.deb` and remove it with
`sudo apt remove wiringcraft`.

Build it locally on Linux (Rust stable, `dpkg-deb`). There is no script for it: run the commands
of the "Package (Linux)" step in `release.yml`. In short, `cargo build --release -p wc-cli -p wc-app`,
copy the files above into a folder `wiringcraft_<version>_amd64` with `install -D`, write
`DEBIAN/control` as in the step, then:

```sh
dpkg-deb --root-owner-group --build wiringcraft_0.1.0_amd64 dist/wiringcraft-0.1.0-linux-amd64.deb
```

## macOS: .dmg

Built on `macos-latest` as one universal app for Apple silicon and Intel. Output:
`wiringcraft-<version>-macos-universal.dmg`.

What it holds: `WiringCraft.app` and a link to `/Applications`, so the user drags the app there.
Inside the app:

- `Contents/MacOS/wiringcraft-app` (the app's executable) and `Contents/MacOS/wiringcraft` (the
  command line program), each a universal binary made with `lipo`.
- `Contents/Info.plist` from `packaging/macos/Info.plist`, with `@VERSION@` replaced by the
  version. Bundle id `io.github.phobicdotno.wiringcraft`, minimum macOS 11.0.
- `Contents/Resources/`: `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `DISCLAIMER.md`,
  `wiringcraft.icns` and the `library` folder.

Build it locally on a Mac (Rust stable, Xcode command line tools for `lipo`, `codesign` and
`hdiutil`):

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --target "$t" -p wc-cli -p wc-app
done
```

Then run the rest of the "Package (macOS)" step in `release.yml`: build `stage/WiringCraft.app`
with `lipo -create`, the `sed` on `Info.plist` and the copies, sign it ad hoc
(`codesign --force --deep --sign - stage/WiringCraft.app`), add the `Applications` link and run
`hdiutil create -volname WiringCraft -srcfolder stage -format UDZO -ov dist/wiringcraft-<version>-macos-universal.dmg`.

## Where the app finds the starter library

The app looks for a `library` folder with a `symbols` folder in it, in this order
(`find_library` in `crates/wc-app/src/main.rs`):

1. `library` next to the program (the Windows install and the Build downloads)
2. `../share/wiringcraft/library` from the program (the Linux package)
3. `../Resources/library` from the program (the macOS app)
4. the `library` folder of the source checkout it was built from

## Signing

The packages are not signed yet. The macOS app has an ad-hoc signature only. Windows SmartScreen
and macOS Gatekeeper warn on the first start; the release notes say how to get past that.

## Cutting a release

1. Set the version in the root `Cargo.toml` (`[workspace.package]`) and merge that to `main`.
2. Push a tag `v<version>`, for example `git tag v0.1.0 && git push origin v0.1.0`.
3. The Release workflow builds and tests the three packages. When all three pass, the "Draft
   release" job creates a draft GitHub release "WiringCraft v0.1.0" for the tag with the packages
   attached and notes on the signing warnings.
4. Check the draft and publish it by hand on GitHub.

A manual run (Actions > Release > Run workflow) and a pull request run build and test the
packages only. They make no release; the packages are under the run's "Artifacts".
