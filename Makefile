SHELL := /bin/sh
.DEFAULT_GOAL := help

NAME ?= main
PORT ?= 8765
SITE_DIR ?= target/demo/site
OTA_IMAGE ?= target/opencc600-ota.bin
RELEASE_DIR ?= target/release
CC600_PORT ?= /dev/ttyACM0
FLASH_AUTO_CONFIRM ?= 0
PYTEST_ARGS ?=
CARGO_LOCAL_CONFIG := $(wildcard .cargo/config .cargo/config.toml)

ifeq ($(CARGO_LOCAL_CONFIG),)
CARGO_LOCKED ?= --locked
else
CARGO_LOCKED ?=
endif

ifneq ($(FLASH_AUTO_CONFIRM),0)
ifneq ($(FLASH_AUTO_CONFIRM),1)
$(error FLASH_AUTO_CONFIRM must be 0 or 1)
endif
endif

FIRMWARE_ELF := firmware/target/xtensa-esp32s3-none-elf/release/opencc600

.PHONY: help firmware firmware-image flash demo site site-copy preview release prepare-release \
	test coverage test-sim test-hil check

help: ## Show common build and test targets
	@awk 'BEGIN { FS = ":.*## " } /^[a-zA-Z0-9_-]+:.*## / { printf "%-18s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

firmware: ## Build the CC600 firmware ELF
	cd firmware && unset BINDGEN_EXTRA_CLANG_ARGS && . "$$HOME/export-esp.sh" >/dev/null && cargo build --release $(CARGO_LOCKED)

firmware-image: firmware ## Package the firmware for the free OTA slot
	mkdir -p "$(dir $(OTA_IMAGE))"
	espflash save-image --chip esp32s3 --flash-size 16mb "$(FIRMWARE_ELF)" "$(OTA_IMAGE)"

flash: ## Check device, build, then confirm before flashing (FLASH_AUTO_CONFIRM=1 skips prompt)
	python3 tools/flash.py --preflight --port "$(CC600_PORT)"
	$(MAKE) --no-print-directory firmware-image
	@if [ "$(FLASH_AUTO_CONFIRM)" = "1" ]; then \
		echo 'FLASH_AUTO_CONFIRM=1: interactive confirmation skipped.'; \
	else \
		printf 'Flash %s to the CC600 on %s? This boots OpenCC600 and may erase vendor storage after backing it up. [y/N] ' "$(OTA_IMAGE)" "$(CC600_PORT)"; \
		read answer; \
		case "$$answer" in y|Y|yes|YES) ;; *) echo 'Flash cancelled.'; exit 1 ;; esac; \
	fi
	python3 tools/flash.py "$(OTA_IMAGE)" --port "$(CC600_PORT)"

demo: ## Build one browser demo version (NAME=main by default)
	tools/demo/build.sh "$(NAME)"

site: demo ## Build and stage a complete site for local preview
	$(MAKE) --no-print-directory site-copy SITE_DIR="$(SITE_DIR)"

site-copy: ## Stage the already-built demo site (SITE_DIR=target/demo/site by default)
	test -n "$(SITE_DIR)"
	mkdir -p "$(SITE_DIR)/screens"
	cp docs/site/index.html docs/demo/versions.json docs/demo/tourmalet.gpx docs/logo-wide.svg "$(SITE_DIR)/"
	cp -r docs/demo/builds docs/demo/rom "$(SITE_DIR)/"
	cp docs/screens/ride-page1.png docs/screens/ride-page2.png docs/screens/route-map-1km.png docs/screens/climbs-in.png "$(SITE_DIR)/screens/"

preview: site-copy ## Serve the existing demo build locally without rebuilding firmware (PORT=8765 by default)
	python3 -m http.server "$(PORT)" --bind 127.0.0.1 --directory "$(SITE_DIR)"

release: demo ## Build demo files and release assets (NAME=vX.Y.Z RELEASE_DIR=target/release)
	mkdir -p "$(RELEASE_DIR)"
	espflash save-image --chip esp32s3 --flash-size 16mb "$(FIRMWARE_ELF)" "$(RELEASE_DIR)/opencc600-$(NAME).bin"
	cp "docs/demo/builds/$(NAME)/fw/cc600.elf" "$(RELEASE_DIR)/opencc600-$(NAME).elf"
	cd "$(RELEASE_DIR)" && sha256sum "opencc600-$(NAME).bin" "opencc600-$(NAME).elf" > SHA256SUMS

prepare-release: ## Set release versions and lockfiles (VERSION=0.0.4; tracked tree clean, no local patches)
	@test -n "$(VERSION)" || { echo 'usage: make prepare-release VERSION=0.0.4' >&2; exit 2; }
	python3 tools/prepare_release.py "$(VERSION)"

test: ## Run host Rust tests (no hardware)
	@if [ -z "$${BINDGEN_EXTRA_CLANG_ARGS:-}" ] && [ -f "$$HOME/export-esp.sh" ]; then \
		. "$$HOME/export-esp.sh" >/dev/null; \
		for d in "$$LIBCLANG_PATH"/clang/*/include; do \
			if [ -f "$$d/stdbool.h" ]; then clang_include="$$d"; fi; \
		done; \
		if [ -n "$${clang_include:-}" ]; then \
			export BINDGEN_EXTRA_CLANG_ARGS="--target=$$(rustc -vV | sed -n 's/^host: //p') -isystem $$clang_include"; \
		fi; \
	fi; \
	cargo test --workspace $(CARGO_LOCKED)

coverage: ## Run host tests and write an HTML coverage report (requires cargo-llvm-cov)
	@if [ -z "$${BINDGEN_EXTRA_CLANG_ARGS:-}" ] && [ -f "$$HOME/export-esp.sh" ]; then \
		. "$$HOME/export-esp.sh" >/dev/null; \
		for d in "$$LIBCLANG_PATH"/clang/*/include; do \
			if [ -f "$$d/stdbool.h" ]; then clang_include="$$d"; fi; \
		done; \
		if [ -n "$${clang_include:-}" ]; then \
			export BINDGEN_EXTRA_CLANG_ARGS="--target=$$(rustc -vV | sed -n 's/^host: //p') -isystem $$clang_include"; \
		fi; \
	fi; \
	cargo llvm-cov --workspace $(CARGO_LOCKED) --html
	cargo llvm-cov report --workspace $(CARGO_LOCKED) --lcov --output-path target/llvm-cov/lcov.info

test-sim: firmware ## Run end-to-end tests on the native CC600 emulator
	uv run --project tests/hil pytest tests/sim $(PYTEST_ARGS)

test-hil: ## Run tests on a CC600 over USB (CC600_PORT=/dev/ttyACM0)
	uv run --project tests/hil pytest tests/hil --port "$(CC600_PORT)" $(PYTEST_ARGS)

check: test test-sim ## Run host and emulator tests (no physical device)
