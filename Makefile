.PHONY: all build-app

all: build-app

ifeq ($(OS),Windows_NT)
BUNDLE = nsis
else
BUNDLE = app
endif

build-app:
	npm run tauri -- build --bundles $(BUNDLE)
