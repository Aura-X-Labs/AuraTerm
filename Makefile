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
	rm -rf src-tauri/target
	rm -rf dist

help:
	@echo "Makefile targets:"
	@echo "  all     - Build the application (default)"
	@echo "  build   - Build the application"
	@echo "  clean   - Remove build artifacts"
	@echo "  release - Process artifacts and update JSON"
	@echo "  upload  - Upload artifacts to server"
	@echo "  help    - Show this help message"

release:
	@echo "Processing artifacts..."
	@python scripts/release.py

upload:	
	@echo "Uploading ..."
	scp releases/* william@alithon.com:Downloads/AuraTerm/
	ssh william@alithon.com "cp Downloads/AuraTerm/* /home/william/releases/aurax/releases/"
	@echo "Upload complete."
