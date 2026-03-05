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
	@mkdir -p releases
	$(eval VERSION_BASE := $(shell grep "version" src-tauri/tauri.conf.json | head -1 | awk -F\" "{print $$4}" | cut -d. -f1,2))
	$(eval MMDD := $(shell date +%m%d))
	$(eval PATCH := $(shell ls releases/AuraTerm-$(VERSION_BASE).*.$(MMDD)-*.dmg 2>/dev/null | sed -E "s/.*-$(VERSION_BASE)\.([0-9]+)\..*/\1/" | sort -n | tail -1 | awk "{print $$1 + 1}"))
	$(eval PATCH_VAL := $(if $(PATCH),$(PATCH),0))
	$(eval FULL_VERSION := $(VERSION_BASE).$(PATCH_VAL).$(MMDD))
	$(eval ARCH := $(shell uname -m))
	$(eval ARCH_TAURI := $(if $(filter arm64,$(ARCH)),aarch64,$(ARCH)))
	$(eval EXT := $(if $(filter Windows_NT,$(OS)),exe,dmg))
	$(eval SRC_PATH := $(if $(filter dmg,$(EXT)),src-tauri/target/release/bundle/dmg/AuraTerm_*_$(ARCH_TAURI).dmg,src-tauri/target/release/bundle/nsis/AuraTerm_*_$(ARCH_TAURI)-setup.exe))
	$(eval ACTUAL_SRC := $(shell ls $(SRC_PATH) 2>/dev/null | head -1))
	@if [ -z "$(ACTUAL_SRC)" ]; then echo "Error: Artifact not found at $(SRC_PATH)"; exit 1; fi
	$(eval DEST_NAME := AuraTerm-$(FULL_VERSION)-$(ARCH).$(EXT))
	cp "$(ACTUAL_SRC)" releases/$(DEST_NAME)
	cp "$(ACTUAL_SRC)" releases/AuraTerm-latest-$(ARCH).$(EXT)
	$(eval SHA256 := $(shell shasum -a 256 "$(ACTUAL_SRC)" | awk "{print $$1}"))
	$(eval PUBLISH_DATE := $(shell date +%Y-%m-%d))
	@if [ -f releases/auraterm-releases.json ]; then \
		cat releases/auraterm-releases.json | jq ".latest = \"$(FULL_VERSION)\" | .releases = [{\"version\": \"$(FULL_VERSION)\", \"filename\": \"$(DEST_NAME)\", \"platform\": \"$(if $(filter dmg,$(EXT)),macos-$(ARCH),windows-x64)\", \"published_at\": \"$(PUBLISH_DATE)\", \"sha256\": \"$(SHA256)\", \"notes\": \"automated release\"}] + .releases" > releases/auraterm-releases.json.tmp && mv releases/auraterm-releases.json.tmp releases/auraterm-releases.json; \
	else \
		echo "{\"product\": \"AuraTerm\", \"latest\": \"$(FULL_VERSION)\", \"releases\": [{\"version\": \"$(FULL_VERSION)\", \"filename\": \"$(DEST_NAME)\", \"platform\": \"$(if $(filter dmg,$(EXT)),macos-$(ARCH),windows-x64)\", \"published_at\": \"$(PUBLISH_DATE)\", \"sha256\": \"$(SHA256)\", \"notes\": \"automated release\"}]}" > releases/auraterm-releases.json; \
	fi
	@echo "Local release prepared: releases/$(DEST_NAME)"

upload:
	$(eval LATEST_FILE := $(shell ls -t releases/AuraTerm-*.dmg releases/AuraTerm-*.exe 2>/dev/null | head -1))
	@if [ -z "$(LATEST_FILE)" ]; then echo "No artifacts found in releases/"; exit 1; fi
	@echo "Uploading $(notdir $(LATEST_FILE))..."
	scp "$(LATEST_FILE)" "releases/auraterm-releases.json" william@alithon.com:Downloads/AuraTerm/
	ssh william@alithon.com "mv Downloads/AuraTerm/$(notdir $(LATEST_FILE)) /home/william/releases/aurax/releases/ && mv Downloads/AuraTerm/auraterm-releases.json /home/william/releases/aurax/releases/"
	@echo "Upload complete."
