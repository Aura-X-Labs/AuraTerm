.PHONY: all build clean help release repackage upload

all: help

ifeq ($(OS),Windows_NT)
BUNDLE = nsis
else
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
BUNDLE = app,dmg
else
BUNDLE = app
endif
endif

build: clean
	npm run tauri -- build --bundles $(BUNDLE)

clean:
	rm -rf src-tauri/target
	rm -rf dist

help:
	@echo "Makefile targets:"
	@echo "  all       - Build the application (default)"
	@echo "  build     - Build the application"
	@echo "  clean     - Remove build artifacts"
	@echo "  release   - Process artifacts and update JSON (increment patch)"
	@echo "  repackage - Re-package artifacts without incrementing patch"
	@echo "  upload    - Upload artifacts to server"
	@echo "  help      - Show this help message"

release:
	@echo "Processing artifacts (increment patch)..."
	@python scripts/release.py

repackage:
	@echo "Re-packaging artifacts (overwrite existing)..."
	@python scripts/release.py --force

upload:
	@echo "Uploading latest release..."
	@python scripts/upload.py
