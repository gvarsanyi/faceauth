SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

MSGFMT := $(shell command -v msgfmt 2>/dev/null || true)
NPROC := $(shell nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 1)

BUILD_KCM_DIR := build-kcm
KCM_SOURCE := faceauth-kcm

SYSTEMD_SERVICE := systemd/faceauth-daemon.service
PAM_CONFIG_DROPIN := pam-config.d/pam_faceauth
DEBIAN_PAM_CONFIG := pam-configs/faceauth
KDE_FINGERPRINT_PAM := pam.d/kde-fingerprint

FACEAUTH_USER := faceauthd
VIDEO_GROUP := video

NOLOGIN_SHELL := $(firstword $(wildcard /usr/sbin/nologin /sbin/nologin /bin/false))
ifeq ($(strip $(NOLOGIN_SHELL)),)
NOLOGIN_SHELL := /bin/false
endif

PAM_MOD_DIR := $(firstword $(wildcard /usr/lib64/security /usr/lib/security /lib/security))
ifeq ($(strip $(PAM_MOD_DIR)),)
PAM_MOD_DIR := /lib/security
endif

LIBEXEC_DIR := $(if $(wildcard /usr/libexec),/usr/libexec,/usr/lib)
export FACEAUTH_LIBEXEC_DIR := $(LIBEXEC_DIR)

POLKIT_ACTIONS_DIR := /usr/share/polkit-1/actions

# --- Packaging (nfpm) ---
VERSION        := $(shell grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
DIST_DIR       := dist
STAGING_KDE    := packaging/staging-kde
# Architecture string for nfpm (amd64, arm64, ...).  Override with NFPM_ARCH=arm64 make deb.
NFPM_ARCH      ?= $(shell dpkg --print-architecture 2>/dev/null \
                    || rpm --eval '%{_arch}' 2>/dev/null | sed 's/x86_64/amd64/;s/aarch64/arm64/' \
                    || echo amd64)
# PAM module directory used in the packages (distro-specific).
# DEB: multiarch path (requires dpkg-architecture, falls back to x86_64-linux-gnu).
# RPM: /usr/lib64/security on Fedora/RHEL x86_64, /usr/lib/security on openSUSE.
DEB_PAM_MOD_DIR := /usr/lib/$(shell dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null \
                       || echo x86_64-linux-gnu)/security
RPM_PAM_MOD_DIR := $(firstword $(wildcard /usr/lib64/security /usr/lib/security) /usr/lib/security)

.PHONY: all build cli daemon notify pam kcm gui install-kcm \
        compile-po staging-kde deb rpm package \
        clean clean-kcm distclean install uninstall help

all: build

build:
	cargo build --release --manifest-path Cargo.toml
	make kcm

cli:
	cargo build --release --manifest-path Cargo.toml --package faceauth-cli --bin faceauth

daemon:
	cargo build --release --manifest-path Cargo.toml --package faceauth-daemon --bin faceauth-daemon

notify:
	cargo build --release --manifest-path Cargo.toml --package faceauth-notify --bin faceauth-notify

pam:
	cargo build --release --manifest-path Cargo.toml --package faceauth-pam

kcm gui:
	@command -v cmake >/dev/null 2>&1 || { echo "cmake not found (required for the 'kcm' target)"; exit 1; }
	cmake -B $(BUILD_KCM_DIR) $(KCM_SOURCE) -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -Wno-dev
	cmake --build $(BUILD_KCM_DIR) -j$(NPROC)

install-kcm:
	cmake --install $(BUILD_KCM_DIR)
	./install-kde.sh
	install -m 644 $(KCM_SOURCE)/org.faceauth.policy $(POLKIT_ACTIONS_DIR)/org.faceauth.policy

compile-po:
	@if [ -z "$(MSGFMT)" ]; then \
	    echo "msgfmt not found -- skipping translations"; \
	else \
	    for po in po/*.po; do \
	        [ -f "$$po" ] || continue; \
	        lang=$$(basename "$$po" .po); \
	        mkdir -p packaging/locale/$$lang/LC_MESSAGES; \
	        $(MSGFMT) -o packaging/locale/$$lang/LC_MESSAGES/faceauth.mo "$$po"; \
	    done; \
	fi

staging-kde: kcm
	rm -rf $(STAGING_KDE)
	@touch $(BUILD_KCM_DIR)/install_manifest.txt 2>/dev/null || \
	    sudo chown "$(shell id -u):$(shell id -g)" $(BUILD_KCM_DIR)/install_manifest.txt
	DESTDIR=$(abspath $(STAGING_KDE)) cmake --install $(BUILD_KCM_DIR)
	install -D -m 644 $(KCM_SOURCE)/org.faceauth.policy \
	    $(STAGING_KDE)/usr/share/polkit-1/actions/org.faceauth.policy

deb: build compile-po staging-kde
	@command -v nfpm >/dev/null 2>&1 || { echo "nfpm not found -- install from https://nfpm.goreleaser.com"; exit 1; }
	mkdir -p $(DIST_DIR)
	FACEAUTH_VERSION=$(VERSION) \
	FACEAUTH_LIBEXEC_DIR=/usr/libexec \
	FACEAUTH_PAM_MOD_DIR=$(DEB_PAM_MOD_DIR) \
	NFPM_ARCH=$(NFPM_ARCH) \
	nfpm package -f packaging/nfpm-main.yaml -t $(DIST_DIR) -p deb
	FACEAUTH_VERSION=$(VERSION) \
	NFPM_ARCH=$(NFPM_ARCH) \
	nfpm package -f packaging/nfpm-kde.yaml -t $(DIST_DIR) -p deb

rpm: build compile-po staging-kde
	@command -v nfpm >/dev/null 2>&1 || { echo "nfpm not found -- install from https://nfpm.goreleaser.com"; exit 1; }
	mkdir -p $(DIST_DIR)
	FACEAUTH_VERSION=$(VERSION) \
	FACEAUTH_LIBEXEC_DIR=/usr/libexec \
	FACEAUTH_PAM_MOD_DIR=$(RPM_PAM_MOD_DIR) \
	NFPM_ARCH=$(NFPM_ARCH) \
	nfpm package -f packaging/nfpm-main.yaml -t $(DIST_DIR) -p rpm
	FACEAUTH_VERSION=$(VERSION) \
	NFPM_ARCH=$(NFPM_ARCH) \
	nfpm package -f packaging/nfpm-kde.yaml -t $(DIST_DIR) -p rpm

package: deb rpm

install:
	@command -v systemctl >/dev/null 2>&1 || { echo "systemctl is required to install faceauth"; exit 1; }
	# install binaries
	install -m 755 target/release/faceauth /usr/bin/faceauth
	install -m 755 target/release/faceauth-daemon $(FACEAUTH_LIBEXEC_DIR)/faceauth-daemon
	install -m 755 target/release/faceauth-notify $(FACEAUTH_LIBEXEC_DIR)/faceauth-notify
	install -d -m 755 $(PAM_MOD_DIR)
	install -m 644 target/release/libpam_faceauth.so $(PAM_MOD_DIR)/pam_faceauth.so
	useradd --system --no-create-home --shell $(NOLOGIN_SHELL) --groups $(VIDEO_GROUP) $(FACEAUTH_USER) || true
	usermod -aG $(VIDEO_GROUP) $(FACEAUTH_USER)
	# Setting up model directory
	mkdir -p /etc/security/faceauth
	chown $(FACEAUTH_USER):$(FACEAUTH_USER) /etc/security/faceauth
	chmod 750 /etc/security/faceauth
	# Installing systemd service
	install -m 644 $(SYSTEMD_SERVICE) /etc/systemd/system/faceauth-daemon.service
	if [ "$(FACEAUTH_LIBEXEC_DIR)" != "/usr/libexec" ]; then \
        sed -i "s|/usr/libexec/|$(FACEAUTH_LIBEXEC_DIR)/|g" /etc/systemd/system/faceauth-daemon.service; \
	fi
	systemctl daemon-reload
	systemctl enable faceauth-daemon
	systemctl restart faceauth-daemon
	# Installing translations (if available)
	for po in po/*.po; do \
		[ -f "$$po" ] || continue; \
		lang=$$(basename "$$po" .po); \
		locale_dir="/usr/share/locale/$$lang/LC_MESSAGES"; \
		mkdir -p "$$locale_dir"; \
		msgfmt -o "$$locale_dir/faceauth.mo" "$$po"; \
	done
	# Install PAM configuration
	./install-pam.sh
	# Install KDE plugin & KCM
	make install-kcm
	@echo ""
	@echo "Done. faceauth-daemon is running."
	@echo "Enroll your face with:"
	@echo "    faceauth add"

uninstall:
	systemctl disable --now faceauth-daemon || true
	rm -f /etc/systemd/system/faceauth-daemon.service
	systemctl daemon-reload
	rm -f /usr/bin/faceauth
	rm -f $(FACEAUTH_LIBEXEC_DIR)/faceauth-daemon
	rm -f $(FACEAUTH_LIBEXEC_DIR)/faceauth-notify
	rm -f $(PAM_MOD_DIR)/pam_faceauth.so
	if [ -d /usr/share/locale ]; then \
		for mo in /usr/share/locale/*/LC_MESSAGES/faceauth.mo; do \
			[ -f "$$mo" ] && rm -f "$$mo"; \
		done; \
	fi
	pam-auth-update --remove faceauth >/dev/null 2>&1 || true
	rm -f /usr/lib/pam-config.d/pam_faceauth || true
	rm -f /usr/share/pam-configs/faceauth || true
	rm -f /etc/security/faceauth || true
	rm -f /etc/pam.d/kde-fingerprint || true

clean:
	cargo clean
	rm -rf $(BUILD_KCM_DIR) $(STAGING_KDE) packaging/locale $(DIST_DIR)

help:
	@echo "Common targets:"
	@echo "  make            Build all components (release)"
	@echo "  make build      Build all components (release)"
	@echo "  make cli        Build the CLI binary"
	@echo "  make daemon     Build the daemon binary"
	@echo "  make notify     Build the notification helper"
	@echo "  make pam        Build the PAM module"
	@echo "  make kcm        Configure & build the KDE System Settings module"
	@echo "  make install    Install faceauth"
	@echo "  make uninstall  Remove installed files"
	@echo "  make clean      Clean Cargo artifacts, CMake build dir, and packaging artifacts"
	@echo ""
	@echo "Packaging targets (require nfpm: https://nfpm.goreleaser.com):"
	@echo "  make deb        Build faceauth and faceauth-kde .deb packages -> dist/"
	@echo "  make rpm        Build faceauth and faceauth-kde .rpm packages -> dist/"
	@echo "  make package    Build both deb and rpm packages"
	@echo ""
	@echo "  NFPM_ARCH=arm64 make deb   Override target architecture"
