# Build dependencies

Rust 1.88 or newer (via rustup or the distribution) is required; the GTK app
needs Rust 1.92. The KDE module needs KF6 6.8 or newer, the GTK app GTK 4.14
and libadwaita 1.5. Runtime requirements: systemd, D-Bus, polkit, Linux-PAM, dlib, and
a V4L2 camera (preferably IR).

## openSUSE Tumbleweed / Leap

```bash
# Daemon, PAM module, CLI
sudo zypper install gcc-c++ dlib-devel clang-devel kernel-headers libv4l-devel pam-devel gettext-tools
# KDE System Settings module and faceauth-gui (optional, KCM=0 skips)
sudo zypper install cmake kf6-extra-cmake-modules kf6-kcmutils-devel kf6-kcoreaddons-devel \
    kf6-ki18n-devel kf6-kirigami-devel qt6-quick-devel qt6-quickcontrols2-devel qt6-dbus-devel
# GTK app faceauth-settings (optional; built automatically when present)
sudo zypper install gtk4-devel libadwaita-devel
```

## Fedora

```bash
sudo dnf install gcc-c++ dlib-devel clang-devel kernel-headers libv4l-devel pam-devel gettext
sudo dnf install cmake extra-cmake-modules kf6-kcmutils-devel kf6-kcoreaddons-devel \
    kf6-ki18n-devel kf6-kirigami-devel qt6-qtdeclarative-devel qt6-qtbase-devel
sudo dnf install gtk4-devel libadwaita-devel
```

## Debian / Ubuntu

```bash
sudo apt install build-essential libdlib-dev libblas-dev liblapack-dev libclang-dev \
    linux-libc-dev libv4l-dev libpam0g-dev gettext
sudo apt install cmake extra-cmake-modules libkf6kcmutils-dev libkf6coreaddons-dev \
    libkf6i18n-dev libkf6kirigami-dev qt6-declarative-dev qt6-base-dev
sudo apt install libgtk-4-dev libadwaita-1-dev
```

## Arch Linux

```bash
sudo pacman -S base-devel clang linux-api-headers v4l-utils pam gettext
yay -S dlib            # AUR
sudo pacman -S cmake extra-cmake-modules kcmutils kcoreaddons ki18n kirigami qt6-declarative
sudo pacman -S gtk4 libadwaita
```

## What each dependency is for

| Dependency | Used by |
|---|---|
| C++ compiler, dlib, BLAS/LAPACK | `dlib-face-recognition` (vendored) compiles C++ glue against dlib |
| libclang | `bindgen` generates the V4L2 bindings used by nokhwa |
| kernel / libc V4L2 headers, libv4l | camera capture |
| PAM headers | the PAM module links libpam |
| gettext (`msgfmt`) | compiles translations during `make install` |
| cmake, ECM, KF6, Qt 6 | the KDE System Settings module and `faceauth-gui` |
| GTK 4, libadwaita | `faceauth-settings` (GNOME, COSMIC, others) |

dlib 19.24 and 20.x are both supported. The dlib face models are part of the
repository (`faceauth-core/models/`); the build needs no network access beyond
fetching crates (use `cargo vendor` for fully offline builds).

## Packaging (`make deb` / `make rpm`)

Requires [nfpm](https://nfpm.goreleaser.com). openSUSE: `zypper install nfpm`;
Arch: `nfpm-bin` (AUR); Debian/Fedora: the GoReleaser apt/dnf repositories.
