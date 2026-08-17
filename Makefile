# Blue Ear local targets.
#
# Whisper is on by default (Cargo default feature + tauri.conf.json
# `build.features`). That build needs CMake and a C++ compiler on macOS and
# Windows. Tauri always passes `--no-default-features`, then re-enables
# whatever is in `build.features`. The *-no-whisper targets merge-patch that
# array to empty so Whisper.cpp is not linked.

.PHONY: help install dev dev-no-whisper build build-no-whisper \
	build-macos build-windows test test-frontend test-rust \
	test-rust-no-whisper live-test clean

NO_WHISPER_CONFIG = {"build":{"features":[]}}

help:
	@echo "install                 npm install"
	@echo "dev                     tauri dev (Whisper on; needs cmake)"
	@echo "dev-no-whisper          tauri dev without Whisper.cpp"
	@echo "build                   tauri build (Whisper on; needs cmake)"
	@echo "build-no-whisper        tauri build without Whisper.cpp"
	@echo "build-macos             tauri build --bundles app"
	@echo "build-windows           tauri build --bundles nsis"
	@echo "test                    frontend + rust tests (Whisper on)"
	@echo "test-frontend           npm test"
	@echo "test-rust               cargo test (Whisper on; needs cmake)"
	@echo "test-rust-no-whisper    cargo test --no-default-features"
	@echo "live-test               ignored cargo tests (running Teams or Zoom)"
	@echo "clean                   remove dist, node_modules, and build artifacts"

install:
	npm install

dev:
	npm run tauri dev

dev-no-whisper:
	npm run tauri -- dev --config '$(NO_WHISPER_CONFIG)'

build:
	npm run tauri build

build-no-whisper:
	npm run tauri -- build --config '$(NO_WHISPER_CONFIG)'

build-macos:
	npm run tauri build -- --bundles app

build-windows:
	npm run tauri build -- --bundles nsis

test: test-frontend test-rust

test-frontend:
	npm test

test-rust:
	cargo test --manifest-path src-tauri/Cargo.toml

test-rust-no-whisper:
	cargo test --manifest-path src-tauri/Cargo.toml --no-default-features

live-test:
	cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture live_

clean:
	rm -rf dist node_modules src-tauri/target \
		src-tauri/native/BlueEarAudio/.build \
		src-tauri/native/BlueEarAudio/.swiftpm
