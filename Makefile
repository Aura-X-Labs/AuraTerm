.PHONY: all build clean help release upload

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
	@python -c "import shutil; [shutil.rmtree(p, ignore_errors=True) for p in ('src-tauri/target', 'dist')]"

help:
	@echo "Makefile targets:"
	@echo "  all       - Build the application (default)"
	@echo "  build     - Build the application"
	@echo "  clean     - Remove build artifacts"
	@echo "  release   - Update releases JSON metadata from target artifacts"
	@echo "  upload    - Run release then upload target artifacts and JSON"
	@echo "  help      - Show this help message"

release:
	@echo "Updating release metadata JSON..."
	@python scripts/release.py

upload: release
	@echo "Uploading target artifacts..."
	@python scripts/upload.py
