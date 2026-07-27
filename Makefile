# mtg-deck-snap — developer convenience targets.

.PHONY: help setup install-deps fetch-inputs build test check fmt clippy

help:
	@echo "Targets:"
	@echo "  make install-deps  Ensure uv + gdown are installed (for examples/fetch-inputs.sh)"
	@echo "  make setup         Alias for install-deps"
	@echo "  make fetch-inputs  Download example input images into examples/inputs/"
	@echo "  make build         cargo build --release"
	@echo "  make test          cargo test --release"
	@echo "  make check         build + test + clippy + fmt-check (mirrors CI)"

# Ensure the tooling the fetch script needs is present. Idempotent: skips
# anything already installed.
install-deps:
	@if command -v uv >/dev/null 2>&1; then \
		echo "uv already installed: $$(uv --version)"; \
	else \
		echo "Installing uv via official installer..."; \
		curl -LsSf https://astral.sh/uv/install.sh | sh; \
	fi
	@if command -v gdown >/dev/null 2>&1; then \
		echo "gdown already installed: $$(gdown --version 2>/dev/null || echo present)"; \
	else \
		echo "Installing gdown via uv..."; \
		uv tool install gdown; \
	fi
	@echo "Dependencies ready. Run 'make fetch-inputs' to download example images."

setup: install-deps

fetch-inputs:
	./examples/fetch-inputs.sh

build:
	cargo build --release

test:
	cargo test --release

fmt:
	cargo fmt --all

clippy:
	cargo clippy --release --all-targets -- -D warnings

# Mirror the CI gate locally.
check: build test clippy
	cargo fmt --all --check
