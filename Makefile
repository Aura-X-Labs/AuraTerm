.PHONY: all build-app

all: build-app

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

build-app:
	npm run tauri -- build --bundles $(BUNDLE)

clean:
	npm run tauri -- clean
	rm -rf src-tauri/target
