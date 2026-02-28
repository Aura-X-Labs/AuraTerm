.PHONY: all build-app

all: build-app

build-app:
	npm run tauri -- build --bundles app
