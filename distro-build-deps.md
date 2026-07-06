# Build Dependencies

This document lists the system packages required to build faceauth as of early 2026.

## openSUSE Tumbleweed (and Leap)

### Rust CLI + daemon + PAM module
```bash
sudo zypper install gcc-c++ cmake dlib-devel kernel-headers libv4l-devel clang-devel pam-devel rsvg-convert ImageMagick
```

| Package | Purpose |
|---|---|
| `gcc-c++` | C++ compiler required by `dlib-face-recognition` build script |
| `cmake` | Required to build dlib |
| `dlib-devel` | dlib C++ headers and library |
| `kernel-headers` | `linux/videodev2.h` for V4L2 bindings (`v4l2-sys-mit`) |
| `libv4l-devel` | Userspace V4L2 library headers |
| `clang-devel` | Provides `libclang.so`, required by `bindgen` to generate V4L2 bindings |
| `pam-devel` | PAM headers and `libpam.so` for the PAM module |
| `rsvg-convert` | `rsvg-convert`: renders SVG to PNG for the app icon |
| `ImageMagick` | `magick`: converts PNG to ICO |

### KDE KCM + standalone GUI (`faceauth-kcm`)
```bash
sudo zypper install kf6-extra-cmake-modules kf6-kcmutils-devel kf6-kcoreaddons-devel kf6-ki18n-devel kf6-kirigami-devel kf6-kauth-devel qt6-quick-devel qt6-quickcontrols2-devel qt6-network-devel
```

| Package | Purpose |
|---|---|
| `kf6-extra-cmake-modules` | ECM: KDE's CMake module collection (`KDEInstallDirs`, etc.) |
| `kf6-kcmutils-devel` | `KQuickConfigModule`, `kcmutils_add_qml_kcm` macro |
| `kf6-kcoreaddons-devel` | `KPluginFactory`, `KAboutData` |
| `kf6-ki18n-devel` | `KLocalizedString` (`i18n()`) |
| `kf6-kirigami-devel` | Kirigami QML components |
| `kf6-kauth-devel` | `KAuth::Action`, `KAuth::ExecuteJob` for polkit step-up auth |
| `qt6-quick-devel` | Qt Quick / QML |
| `qt6-quickcontrols2-devel` | `QQuickStyle` for Breeze theme in standalone binary |
| `qt6-network-devel` | `QLocalSocket` |

---

## Debian / Ubuntu (and derivatives: Mint, Pop!\_OS, elementary OS, etc.)

### Rust CLI + daemon + PAM module
```bash
sudo apt install build-essential cmake libdlib-dev linux-headers-generic libv4l-dev libclang-dev libpam0g-dev librsvg2-bin imagemagick
```

| Package | Purpose |
|---|---|
| `build-essential` | C/C++ compiler (`gcc`, `g++`) and base build tools |
| `cmake` | Required to build dlib |
| `libdlib-dev` | dlib C++ headers and library |
| `linux-headers-generic` | Kernel headers for V4L2 bindings |
| `libv4l-dev` | Userspace V4L2 library headers |
| `libclang-dev` | Provides `libclang.so`, required by `bindgen` to generate V4L2 bindings |
| `libpam0g-dev` | PAM headers and `libpam.so` for the PAM module |
| `librsvg2-bin` | `rsvg-convert`: renders SVG to PNG for the app icon |
| `imagemagick` | `magick`: converts PNG to ICO |

> **Note for Ubuntu:** `linux-headers-generic` installs headers for the default kernel. If you are running a non-generic kernel (e.g. `linux-oem-*`, `linux-lowlatency`), replace it with `linux-headers-$(uname -r)`.

### KDE KCM + standalone GUI (`faceauth-kcm`)
```bash
sudo apt install extra-cmake-modules libkf6kcmutils-dev libkf6coreaddons-dev libkf6i18n-dev kirigami2-dev libkf6kirigami2-dev libkf6auth-dev qt6-declarative-dev qt6-quickcontrols2-dev libqt6network6-dev
```

| Package | Purpose |
|---|---|
| `extra-cmake-modules` | ECM: KDE's CMake module collection |
| `libkf6kcmutils-dev` | `KQuickConfigModule`, `kcmutils_add_qml_kcm` macro |
| `libkf6coreaddons-dev` | `KPluginFactory`, `KAboutData` |
| `libkf6i18n-dev` | `KLocalizedString` (`i18n()`) |
| `kirigami2-dev` / `libkf6kirigami2-dev` | Kirigami QML components |
| `libkf6auth-dev` | `KAuth::Action`, `KAuth::ExecuteJob` for polkit step-up auth |
| `qt6-declarative-dev` | Qt Quick / QML |
| `qt6-quickcontrols2-dev` | `QQuickStyle` for Breeze theme in standalone binary |
| `libqt6network6-dev` | `QLocalSocket` |

---

## Arch Linux (and derivatives: Manjaro, EndeavourOS, Garuda, etc.)

### Rust CLI + daemon + PAM module
```bash
sudo pacman -S base-devel cmake linux-headers libv4l clang pam librsvg imagemagick
```

For dlib, install from the AUR:

```bash
# using yay
yay -S dlib

# or using paru
paru -S dlib
```

| Package | Purpose |
|---|---|
| `base-devel` | C/C++ compiler and base build tools |
| `cmake` | Required to build dlib |
| `dlib` (AUR) | dlib C++ headers and library |
| `linux-headers` | Kernel headers for V4L2 bindings |
| `libv4l` | Userspace V4L2 library headers |
| `clang` | Provides `libclang.so`, required by `bindgen` to generate V4L2 bindings |
| `pam` | PAM headers and `libpam.so` for the PAM module (includes dev files) |
| `librsvg` | `rsvg-convert`: renders SVG to PNG for the app icon |
| `imagemagick` | `magick`: converts PNG to ICO |

> **Note for Manjaro:** Replace `linux-headers` with the headers package matching your kernel, e.g. `linux61-headers`. You can find your kernel version with `mhwd-kernel -li`.

### KDE KCM + standalone GUI (`faceauth-kcm`)
```bash
sudo pacman -S extra-cmake-modules kcmutils kcoreaddons ki18n kirigami kauth qt6-declarative qt6-quickcontrols2
```

| Package | Purpose |
|---|---|
| `extra-cmake-modules` | ECM: KDE's CMake module collection |
| `kcmutils` | `KQuickConfigModule`, `kcmutils_add_qml_kcm` macro |
| `kcoreaddons` | `KPluginFactory`, `KAboutData` |
| `ki18n` | `KLocalizedString` (`i18n()`) |
| `kirigami` | Kirigami QML components |
| `kauth` | `KAuth::Action`, `KAuth::ExecuteJob` for polkit step-up auth |
| `qt6-declarative` | Qt Quick / QML |
| `qt6-quickcontrols2` | `QQuickStyle` for Breeze theme in standalone binary |

---

## Packaging only (`make deb` / `make rpm`)

### nfpm

**openSUSE Tumbleweed:**
```bash
sudo zypper install nfpm
```

**Arch Linux (AUR):**
```bash
yay -S nfpm-bin   # or: paru -S nfpm-bin
```

**Debian / Ubuntu** -- not in the official repos; use the upstream apt repository:
```bash
echo 'deb [trusted=yes] https://repo.goreleaser.com/apt/ /' | sudo tee /etc/apt/sources.list.d/goreleaser.list
sudo apt update && sudo apt install nfpm
```

**Fedora / RHEL** -- not in the official repos; use the upstream dnf repository:
```bash
echo -e '[goreleaser]\nname=GoReleaser\nbaseurl=https://repo.goreleaser.com/yum/\nenabled=1\ngpgcheck=0' | sudo tee /etc/yum.repos.d/goreleaser.repo
sudo dnf install nfpm
```

### Translations (optional)

`msgfmt` compiles `.po` files to `.mo` at package build time. Skipped silently if absent.

| Distro | Package |
|---|---|
| openSUSE | `sudo zypper install gettext-tools` |
| Debian / Ubuntu | `sudo apt install gettext` |
| Arch Linux | `sudo pacman -S gettext` |
