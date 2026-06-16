## Installation

### macOS

Homebrew is the preferred macOS install method:

```sh
brew install --cask fhsgoncalves/tap/sqlab
```

This installs the `sqlab.app` bundle into `Applications`.

sq/lab is ad-hoc signed but not Developer ID signed or notarized. If macOS reports that `sqlab` is damaged after installation, remove the quarantine attribute:

```sh
xattr -dr com.apple.quarantine /Applications/sqlab.app
```

You can also download `sqlab-aarch64-apple-darwin.dmg`, open it, and drag `sqlab.app` to `Applications`.

The shell installer is still available for CLI-style installs, but it installs only the raw binary into `~/.cargo/bin` and does not provide the macOS app bundle icon.

### Windows

Download `sqlab-x86_64-pc-windows-msvc.msi` and run it.

The MSI installs sqlab under `Program Files`, adds it to Windows "Add or remove programs", and supports upgrades between releases.

### Linux

For Flatpak, download the bundle for your CPU:

- `sqlab-x86_64-unknown-linux-gnu.flatpak` for x64 Linux
- `sqlab-aarch64-unknown-linux-gnu.flatpak` for ARM64 Linux

Install it with:

```sh
flatpak install --user ./sqlab-*-unknown-linux-gnu.flatpak
flatpak run io.github.fhsgoncalves.sqlab
```

For CLI-style installs, the shell installer and tarballs are also available.
