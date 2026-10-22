WIN_TARGET := x86_64-pc-windows-msvc
LINUX_TARGET := x86_64-unknown-linux-gnu
WIN_EXE    := target/$(WIN_TARGET)/release/dost.exe
# The exe is copied to the Windows disk: running it from the WSL file system (\\wsl$) is slow
# and can lock the file.
WIN_DIR     = $(shell wslpath "$$(cmd.exe /c 'echo %LOCALAPPDATA%' 2>/dev/null | tr -d '\r')")/dost

MAC_TARGETS := aarch64-apple-darwin x86_64-apple-darwin
APP         := target/Dost.app
VERSION     := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# codesign identity: "-" (ad hoc) or "Developer ID Application: ..." for a release.
SIGN_ID     ?= -

.PHONY: help build build-release test check clippy fmt preview atlas icons install-opencode-plugin \
	build-linux build-windows run-windows stop-windows app dmg notarize clean

help:
	@echo "make build                   Debug build for this computer (target/debug/dost)"
	@echo "make build-release           Release build for this computer (target/release/dost)"
	@echo "make test                    Run the tests"
	@echo "make check                   cargo check for Linux, Windows and macOS"
	@echo "make clippy                  clippy for Linux, Windows and macOS"
	@echo "make fmt                     Format the code"
	@echo "make preview                 Render the dog, bubbles and panel to preview*.png"
	@echo "make atlas                   Rebuild assets/dog.png from art/px"
	@echo "make icons                   (on a Mac) Rebuild the app icons from examples/icon.rs"
	@echo "make build-linux             Release build for Linux (on Linux; elsewhere cargo-zigbuild)"
	@echo "make build-windows           Release build for Windows from WSL ($(WIN_EXE))"
	@echo "make run-windows             Build, copy to Windows and start it"
	@echo "make stop-windows            Stop a running dost.exe"
	@echo "make app                     (on a Mac) Universal Dost.app in target/"
	@echo "make dmg                     (on a Mac) Dost-$(VERSION).dmg in target/"
	@echo "make notarize SIGN_ID=...     (on a Mac) Signed, notarized and stapled dmg"
	@echo "make install-opencode-plugin Install the OpenCode plugin from this checkout"
	@echo "make clean                   Remove target/"

build:
	cargo build
	@echo "-> target/debug/dost"

build-release:
	cargo build --release
	@echo "-> target/release/dost"

test:
	cargo test

check:
	cargo check --all-targets
	cargo xwin check --all-targets --target $(WIN_TARGET)
	cargo check --all-targets --target aarch64-apple-darwin
	cargo check --all-targets --target $(LINUX_TARGET)

clippy:
	cargo clippy --all-targets -- -D warnings
	cargo xwin clippy --all-targets --target $(WIN_TARGET) -- -D warnings
	cargo clippy --all-targets --target aarch64-apple-darwin -- -D warnings
	cargo clippy --all-targets --target $(LINUX_TARGET) -- -D warnings

fmt:
	cargo fmt

preview:
	cargo run --example preview preview.png

# art/px/* (Sprite Forge pixel finish) -> assets/dog.png
atlas:
	~/tools/agent-sprite-forge/.venv/bin/python art/pack_atlas.py
	~/tools/agent-sprite-forge/.venv/bin/python art/pack_items.py
	~/tools/agent-sprite-forge/.venv/bin/python art/collar.py

# examples/icon.rs -> packaging/macos/AppIcon.icns and packaging/windows/dost.ico.
ICON_TMP := target/icons
icons:
	@test "$$(uname)" = Darwin || { echo "make icons needs macOS (iconutil)"; exit 1; }
	rm -rf $(ICON_TMP) && mkdir -p $(ICON_TMP)/AppIcon.iconset packaging/windows
	cargo run -q --example icon $(ICON_TMP)
	for s in 16 32 128 256 512; do \
		cp $(ICON_TMP)/icon_$$s.png $(ICON_TMP)/AppIcon.iconset/icon_$${s}x$${s}.png; \
		cp $(ICON_TMP)/icon_$$((s*2)).png $(ICON_TMP)/AppIcon.iconset/icon_$${s}x$${s}@2x.png; \
	done
	iconutil -c icns $(ICON_TMP)/AppIcon.iconset -o packaging/macos/AppIcon.icns
	magick $(foreach s,16 20 24 32 48 64 128 256,$(ICON_TMP)/icon_$(s).png) packaging/windows/dost.ico
	@echo "-> packaging/macos/AppIcon.icns, packaging/windows/dost.ico"

# From macOS, ring (rustls) needs a C cross compiler for the Linux target: cargo-zigbuild does it.
build-linux:
	@rustup target list --installed | grep -qx $(LINUX_TARGET) || rustup target add $(LINUX_TARGET)
	@if [ "$$(uname)" = Linux ]; then cargo build --release --target $(LINUX_TARGET); \
	else cargo zigbuild --release --target $(LINUX_TARGET); fi
	@echo "-> target/$(LINUX_TARGET)/release/dost"

build-windows:
	@command -v cargo-xwin >/dev/null || { echo "cargo-xwin is missing: cargo install --locked cargo-xwin"; exit 1; }
	@rustup target list --installed | grep -qx $(WIN_TARGET) || rustup target add $(WIN_TARGET)
	cargo xwin build --release --target $(WIN_TARGET)
	@echo "-> $(WIN_EXE)"

run-windows: build-windows stop-windows
	mkdir -p "$(WIN_DIR)"
	cp $(WIN_EXE) "$(WIN_DIR)/dost.exe"
	cd "$(WIN_DIR)" && (./dost.exe >/dev/null 2>&1 &)
	@echo "Dost is running: $(WIN_DIR)/dost.exe"

stop-windows:
	-@taskkill.exe /IM dost.exe /F >/dev/null 2>&1 || true

# macOS: a universal (Apple Silicon + Intel) menu bar app. Run on a Mac.
app:
	@test "$$(uname)" = Darwin || { echo "make app needs macOS"; exit 1; }
	rustup target add $(MAC_TARGETS)
	$(foreach t,$(MAC_TARGETS),cargo build --release --target $(t) &&) true
	rm -rf $(APP)
	mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources
	lipo -create $(foreach t,$(MAC_TARGETS),target/$(t)/release/dost) -output $(APP)/Contents/MacOS/dost
	sed 's/@VERSION@/$(VERSION)/g' packaging/macos/Info.plist > $(APP)/Contents/Info.plist
	cp packaging/macos/AppIcon.icns $(APP)/Contents/Resources/
	codesign --force --deep --options runtime --entitlements packaging/macos/Dost.entitlements $(if $(filter -,$(SIGN_ID)),--timestamp=none,--timestamp) --sign "$(SIGN_ID)" $(APP)
	codesign --verify --strict --verbose=2 $(APP)
	@echo "-> $(APP)"

dmg: app
	rm -f target/Dost-$(VERSION).dmg
	hdiutil create -volname Dost -srcfolder $(APP) -ov -format UDZO target/Dost-$(VERSION).dmg
	$(if $(filter -,$(SIGN_ID)),,codesign --sign "$(SIGN_ID)" --timestamp target/Dost-$(VERSION).dmg)
	@echo "-> target/Dost-$(VERSION).dmg"

# Notarizes and staples the dmg and the app (needs a Developer ID SIGN_ID and a notarytool
# profile: xcrun notarytool store-credentials dost --apple-id ... --team-id ...).
NOTARY_PROFILE ?= dost
notarize: dmg
	@test "$(SIGN_ID)" != "-" || { echo "notarize needs SIGN_ID=\"Developer ID Application: ...\""; exit 1; }
	xcrun notarytool submit target/Dost-$(VERSION).dmg --keychain-profile $(NOTARY_PROFILE) --wait
	xcrun stapler staple target/Dost-$(VERSION).dmg
	xcrun stapler staple $(APP)
	spctl --assess --type open --context context:primary-signature -v target/Dost-$(VERSION).dmg

OPENCODE_PLUGINS ?= $(or $(XDG_CONFIG_HOME),$(HOME)/.config)/opencode/plugins

install-opencode-plugin:
	mkdir -p "$(OPENCODE_PLUGINS)"
	cp integrations/opencode/index.js "$(OPENCODE_PLUGINS)/dost.js"
	@echo "-> $(OPENCODE_PLUGINS)/dost.js (OpenCode reloads it by itself)"

clean:
	cargo clean
