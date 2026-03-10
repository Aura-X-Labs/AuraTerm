.PHONY: all build clean help release upload

all: help

ifeq ($(OS),Windows_NT)
BUNDLE = nsis
# For Windows, we use powershell to time the commands. Use $$ to escape $ for Make.
TIME_PREFIX = powershell -ExecutionPolicy Bypass -Command "$$s=Get-Date;
TIME_SUFFIX = ; $$e=Get-Date; Write-Host \"`nExecution time: $$($$e-$$s)\""
else
UNAME_S := $(shell uname -s)
# For Unix, we use /usr/bin/time
TIME_PREFIX = /usr/bin/time -p
TIME_SUFFIX =
ifeq ($(UNAME_S),Darwin)
BUNDLE = app,dmg
else
BUNDLE = app
endif
endif

build: clean
	@$(if $(filter Windows_NT,$(OS)),$(TIME_PREFIX) npm run tauri -- build --bundles $(BUNDLE) $(TIME_SUFFIX),$(TIME_PREFIX) npm run tauri -- build --bundles $(BUNDLE))

clean:
	@$(if $(filter Windows_NT,$(OS)),$(TIME_PREFIX) python -c \"import shutil; [shutil.rmtree(p, ignore_errors=True) for p in ('src-tauri/target', 'dist')]\" $(TIME_SUFFIX),$(TIME_PREFIX) python -c "import shutil; [shutil.rmtree(p, ignore_errors=True) for p in ('src-tauri/target', 'dist')]")

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
	@$(if $(filter Windows_NT,$(OS)),$(TIME_PREFIX) python scripts/release.py $(TIME_SUFFIX),$(TIME_PREFIX) python scripts/release.py)

upload: release
	@echo "Uploading target artifacts..."
	@$(if $(filter Windows_NT,$(OS)),$(TIME_PREFIX) python scripts/upload.py $(TIME_SUFFIX),$(TIME_PREFIX) python scripts/upload.py)
