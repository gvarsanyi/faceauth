SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

# --- Installation layout (override on the command line) ---
PREFIX        ?= /usr
BINDIR        ?= $(PREFIX)/bin
LIBEXECDIR    ?= $(PREFIX)/libexec
DATADIR       ?= $(PREFIX)/share
MANDIR        ?= $(DATADIR)/man
LOCALEDIR     ?= $(DATADIR)/locale
SYSCONFDIR    ?= /etc
UNITDIR       ?= $(PREFIX)/lib/systemd/system
SYSUSERSDIR   ?= $(PREFIX)/lib/sysusers.d
DBUSPOLICYDIR ?= $(DATADIR)/dbus-1/system.d
DBUSSERVICEDIR ?= $(DATADIR)/dbus-1/system-services
DBUSIFACEDIR  ?= $(DATADIR)/dbus-1/interfaces
POLKITDIR     ?= $(DATADIR)/polkit-1/actions
PAMCONFIGSDIR ?= $(DATADIR)/pam-configs
# PAM module directory: first existing candidate on the build host.
PAM_MODULE_DIR ?= $(firstword $(wildcard /usr/lib64/security /usr/lib/$(shell gcc -dumpmachine 2>/dev/null)/security /usr/lib/security) /usr/lib/security)
DESTDIR       ?=

# Build the KDE System Settings module and GUI (needs cmake, Qt 6, KF6).
KCM ?= 1
# Build faceauth-settings for GNOME/COSMIC (needs GTK 4.14, libadwaita 1.5).
# Default: only if their development files are installed.
GTK ?= $(shell pkg-config --exists 'gtk4 >= 4.14' 'libadwaita-1 >= 1.5' 2>/dev/null && echo 1 || echo 0)

CARGO     ?= cargo
MSGFMT    ?= msgfmt
NPROC     := $(shell nproc 2>/dev/null || echo 1)
VERSION   := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD_KCM := build-kcm
RELEASE   := target/release

export FACEAUTH_LIBEXEC_DIR := $(LIBEXECDIR)
export FACEAUTH_LOCALE_DIR := $(LOCALEDIR)

# Substitute install paths into *.in templates.
SUBST = sed -e 's|@LIBEXECDIR@|$(LIBEXECDIR)|g' -e 's|@DATADIR@|$(DATADIR)|g' -e 's|@SYSUSERSDIR@|$(SYSUSERSDIR)|g'

.PHONY: all rust kcm gtk test lint pot install install-rust install-kcm install-gtk uninstall \
        deb rpm package clean help

all: rust $(if $(filter 1,$(KCM)),kcm) $(if $(filter 1,$(GTK)),gtk)
ifneq ($(GTK),1)
	@echo "Skipped faceauth-settings (GTK app): gtk4/libadwaita development files not found."
endif

# Workspace selection for test/lint: faceauth-gtk only with GTK=1.
CRATES := --workspace $(if $(filter 1,$(GTK)),,--exclude faceauth-gtk)

rust:
	$(CARGO) build --release --locked

gtk:
	$(CARGO) build --release --locked -p faceauth-gtk

kcm:
	cmake -B $(BUILD_KCM) faceauth-kcm -DCMAKE_BUILD_TYPE=Release \
	    -DCMAKE_INSTALL_PREFIX=$(PREFIX) -DFACEAUTH_VERSION=$(VERSION) -Wno-dev
	cmake --build $(BUILD_KCM) -j$(NPROC)

test:
	$(CARGO) test $(CRATES) --locked

lint:
	$(CARGO) clippy $(CRATES) --all-targets --locked -- -D warnings

# Regenerate the translation templates (keeping their header comments) and
# merge them into every catalog.
XGETTEXT = xgettext --from-code=UTF-8 --package-name=faceauth --package-version=$(VERSION) \
           --msgid-bugs-address=https://github.com/gvarsanyi/faceauth/issues
KDE_KEYWORDS = -ki18n:1 -ki18nc:1c,2 -ki18np:1,2 -ki18ncp:1c,2,3
merge_po = msgmerge -q -o "$(1).tmp" "$(1)" $(2) && msgattrib --no-obsolete -o "$(1)" "$(1).tmp" && rm -f "$(1).tmp"
keep_header = { sed -n '/^\#, fuzzy/q;p' $(1); sed -n '/^\#, fuzzy/,$$p' $(2); } > $(1).new && mv $(1).new $(1) && rm -f $(2)

pot:
	$(XGETTEXT) -L Rust -kgettext -kngettext:1,2 -o po/new.pot $(sort $(wildcard faceauth-*/src/*.rs))
	$(call keep_header,po/faceauth.pot,po/new.pot)
	for po in po/*.po; do $(call merge_po,$$po,po/faceauth.pot); done
	$(XGETTEXT) -C $(KDE_KEYWORDS) -o kcm-cpp.pot faceauth-kcm/src/*.cpp
	$(XGETTEXT) -L JavaScript $(KDE_KEYWORDS) -o kcm-qml.pot faceauth-kcm/src/ui/*.qml
	msgcat --use-first -o faceauth-kcm/po/new.pot kcm-cpp.pot kcm-qml.pot && rm -f kcm-cpp.pot kcm-qml.pot
	$(call keep_header,faceauth-kcm/po/kcm_faceauth.pot,faceauth-kcm/po/new.pot)
	for po in faceauth-kcm/po/*/kcm_faceauth.po; do $(call merge_po,$$po,faceauth-kcm/po/kcm_faceauth.pot); done

# Files only; safe with DESTDIR (used for packaging).
install-rust:
	install -Dm755 $(RELEASE)/faceauth             $(DESTDIR)$(BINDIR)/faceauth
	install -Dm755 $(RELEASE)/faceauth-daemon      $(DESTDIR)$(LIBEXECDIR)/faceauth-daemon
	install -Dm755 $(RELEASE)/faceauth-agent       $(DESTDIR)$(LIBEXECDIR)/faceauth-agent
	install -Dm644 $(RELEASE)/libpam_faceauth.so   $(DESTDIR)$(PAM_MODULE_DIR)/pam_faceauth.so
	$(SUBST) data/faceauth-setup.sh > faceauth-setup.tmp
	install -Dm755 faceauth-setup.tmp              $(DESTDIR)$(LIBEXECDIR)/faceauth-setup
	rm -f faceauth-setup.tmp
	$(SUBST) data/systemd/faceauth-daemon.service.in > faceauth-daemon.service.tmp
	install -Dm644 faceauth-daemon.service.tmp     $(DESTDIR)$(UNITDIR)/faceauth-daemon.service
	rm -f faceauth-daemon.service.tmp
	$(SUBST) data/xdg/faceauth-agent.desktop.in > faceauth-agent.desktop.tmp
	install -Dm644 faceauth-agent.desktop.tmp      $(DESTDIR)$(SYSCONFDIR)/xdg/autostart/faceauth-agent.desktop
	rm -f faceauth-agent.desktop.tmp
	$(SUBST) data/dbus/io.github.gvarsanyi.FaceAuth1.service.in > dbus-service.tmp
	install -Dm644 dbus-service.tmp                $(DESTDIR)$(DBUSSERVICEDIR)/io.github.gvarsanyi.FaceAuth1.service
	rm -f dbus-service.tmp
	install -Dm644 data/dbus/io.github.gvarsanyi.FaceAuth1.conf $(DESTDIR)$(DBUSPOLICYDIR)/io.github.gvarsanyi.FaceAuth1.conf
	install -Dm644 data/dbus/io.github.gvarsanyi.FaceAuth1.Manager.xml $(DESTDIR)$(DBUSIFACEDIR)/io.github.gvarsanyi.FaceAuth1.Manager.xml
	install -Dm644 data/polkit/io.github.gvarsanyi.faceauth.policy $(DESTDIR)$(POLKITDIR)/io.github.gvarsanyi.faceauth.policy
	install -Dm644 data/systemd/faceauth.sysusers  $(DESTDIR)$(SYSUSERSDIR)/faceauth.conf
	install -Dm644 data/pam/kde-fingerprint        $(DESTDIR)$(DATADIR)/faceauth/kde-fingerprint
	install -Dm644 assets/icons/faceauth.svg       $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/faceauth.svg
	install -Dm644 data/pam/debian-pam-configs-faceauth  $(DESTDIR)$(PAMCONFIGSDIR)/faceauth
	@if [ -e "$(DESTDIR)$(SYSCONFDIR)/faceauth/faceauth.conf" ]; then \
	    echo "keeping existing $(DESTDIR)$(SYSCONFDIR)/faceauth/faceauth.conf"; \
	else \
	    install -Dm644 data/faceauth.conf $(DESTDIR)$(SYSCONFDIR)/faceauth/faceauth.conf; \
	fi
	for page in man/*.[0-9]; do \
	    install -Dm644 "$$page" "$(DESTDIR)$(MANDIR)/man$${page##*.}/$${page##*/}"; \
	done
	for po in po/*.po; do \
	    lang=$$(basename "$$po" .po); \
	    install -d "$(DESTDIR)$(LOCALEDIR)/$$lang/LC_MESSAGES"; \
	    $(MSGFMT) -o "$(DESTDIR)$(LOCALEDIR)/$$lang/LC_MESSAGES/faceauth.mo" "$$po"; \
	done

# CMake rewrites install_manifest.txt on every install (also when staging
# packages); keep the system install's list for uninstall.
install-kcm:
	DESTDIR=$(DESTDIR) cmake --install $(BUILD_KCM)
	@if [ -z "$(DESTDIR)" ]; then cp $(BUILD_KCM)/install_manifest.txt $(BUILD_KCM)/system_manifest.txt; fi

install-gtk:
	install -Dm755 $(RELEASE)/faceauth-settings    $(DESTDIR)$(BINDIR)/faceauth-settings
	install -Dm644 data/gtk/io.github.gvarsanyi.faceauth.Settings.desktop \
	    $(DESTDIR)$(DATADIR)/applications/io.github.gvarsanyi.faceauth.Settings.desktop

# Install, then configure the running system (skipped when staging).
install: install-rust $(if $(filter 1,$(KCM)),install-kcm) $(if $(filter 1,$(GTK)),install-gtk)
	@if [ -z "$(DESTDIR)" ]; then \
	    $(LIBEXECDIR)/faceauth-setup configure; \
	    echo; echo "Done. Enroll your face with: faceauth add"; \
	    echo "Check the setup with:         faceauth doctor"; \
	fi

uninstall:
	-$(LIBEXECDIR)/faceauth-setup unconfigure
	-$(LIBEXECDIR)/faceauth-setup purge
	rm -f $(BINDIR)/faceauth $(BINDIR)/faceauth-settings $(LIBEXECDIR)/faceauth-daemon $(LIBEXECDIR)/faceauth-agent \
	      $(DATADIR)/applications/io.github.gvarsanyi.faceauth.Settings.desktop \
	      $(DATADIR)/icons/hicolor/scalable/apps/faceauth.svg \
	      $(SYSCONFDIR)/xdg/autostart/faceauth-agent.desktop \
	      $(LIBEXECDIR)/faceauth-setup $(PAM_MODULE_DIR)/pam_faceauth.so \
	      $(UNITDIR)/faceauth-daemon.service \
	      $(DBUSSERVICEDIR)/io.github.gvarsanyi.FaceAuth1.service \
	      $(DBUSPOLICYDIR)/io.github.gvarsanyi.FaceAuth1.conf \
	      $(DBUSIFACEDIR)/io.github.gvarsanyi.FaceAuth1.Manager.xml \
	      $(POLKITDIR)/io.github.gvarsanyi.faceauth.policy \
	      $(SYSUSERSDIR)/faceauth.conf $(PAMCONFIGSDIR)/faceauth \
	      $(LOCALEDIR)/*/LC_MESSAGES/faceauth.mo
	rm -rf $(DATADIR)/faceauth
	for page in man/*.[0-9]; do rm -f "$(MANDIR)/man$${page##*.}/$${page##*/}"; done
	if [ -f $(BUILD_KCM)/system_manifest.txt ]; then xargs rm -f < $(BUILD_KCM)/system_manifest.txt; fi
	@echo "Kept $(SYSCONFDIR)/faceauth/faceauth.conf and the faceauthd account."
	-systemctl daemon-reload

# --- Packaging (nfpm: https://nfpm.goreleaser.com) ---
NFPM_ARCH ?= $(shell dpkg --print-architecture 2>/dev/null || uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
STAGE     := packaging/staging

define nfpm_package
	rm -rf $(STAGE)
	$(MAKE) install-rust DESTDIR=$(abspath $(STAGE))/main PAM_MODULE_DIR=$(2)
	mkdir -p dist
	$(call nfpm,main,$(1))
	$(if $(filter 1,$(KCM)),DESTDIR=$(abspath $(STAGE))/kde cmake --install $(BUILD_KCM))
	$(if $(filter 1,$(KCM)),$(call nfpm,kde,$(1)))
	$(if $(filter 1,$(GTK)),$(MAKE) install-gtk DESTDIR=$(abspath $(STAGE))/gtk)
	$(if $(filter 1,$(GTK)),$(call nfpm,gtk,$(1)))
endef

# $(1): package (main, kde, gtk); $(2): format (deb, rpm).
nfpm = FACEAUTH_VERSION=$(VERSION) NFPM_ARCH=$(NFPM_ARCH) nfpm package -f packaging/nfpm-$(1).yaml -t dist -p $(2)

deb: all
	$(call nfpm_package,deb,/usr/lib/$(shell dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo x86_64-linux-gnu)/security)

rpm: all
	$(call nfpm_package,rpm,$(firstword $(wildcard /usr/lib64/security) /usr/lib/security))

package: deb rpm

clean:
	$(CARGO) clean
	rm -rf $(BUILD_KCM) $(STAGE) dist

help:
	@echo "Targets:"
	@echo "  make                 build everything (KCM=0 skips the KDE module; GTK=0/1 overrides GTK detection)"
	@echo "  make test            run the test suite"
	@echo "  make lint            run clippy with warnings as errors"
	@echo "  make pot             regenerate translation templates and merge catalogs"
	@echo "  sudo make install    install and configure the system"
	@echo "  make install DESTDIR=/tmp/stage   stage files only (no system changes)"
	@echo "  sudo make uninstall  unconfigure, delete enrolled face data, remove files"
	@echo "  make deb | rpm       build packages into dist/ (needs nfpm)"
	@echo ""
	@echo "Layout variables: PREFIX BINDIR LIBEXECDIR DATADIR SYSCONFDIR UNITDIR"
	@echo "  PAM_MODULE_DIR (detected: $(PAM_MODULE_DIR))"
