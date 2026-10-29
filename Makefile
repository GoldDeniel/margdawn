SHELL := /bin/bash
ROOT  := $(CURDIR)
PREFIX ?= /usr/local

CARGO  := cargo
GRPM   := $(HOME)/.cargo/bin/cargo-generate-rpm

.PHONY: all debug release test rpm deb install uninstall desktop clean

all: release

debug:
	$(CARGO) build

release:
	$(CARGO) build --release

test:
	$(CARGO) test --workspace

# Builds margdawn RPM packages into target/generate-rpm
rpm: release
	$(CARGO) install cargo-generate-rpm --locked --quiet || true
	$(GRPM) -p crates/margdawn-gui

# Deb packages require: cargo install cargo-deb
deb: release
	cargo deb -p margdawn-gui

# Local (non-RPM) install, e.g. PREFIX=~/.local make install
install: release
	install -Dm755 target/release/margdawn     $(DESTDIR)$(PREFIX)/bin/margdawn
	install -Dm755 target/release/margdawn-gui $(DESTDIR)$(PREFIX)/bin/margdawn-gui
	install -Dm644 crates/margdawn-gui/assets/margdawn-gui.desktop $(DESTDIR)$(PREFIX)/share/applications/margdawn-gui.desktop
	install -Dm644 crates/margdawn-gui/assets/margdawn-gui.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/margdawn-gui.svg
	-update-desktop-database -q

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/margdawn $(DESTDIR)$(PREFIX)/bin/margdawn-gui
	rm -f $(DESTDIR)$(PREFIX)/share/applications/margdawn-gui.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/margdawn-gui.svg

clean:
	$(CARGO) clean
