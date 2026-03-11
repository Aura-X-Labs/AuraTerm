.PHONY: all build clean help release upload

all: help

ifeq ($(OS),Windows_NT)
BUNDLE = nsis
# For Windows, we use powershell to time the commands.
# Use $(1) as a placeholder for the command to be executed.
# We use [int] to ensure integer division/truncation for seconds.
TIME_CMD = powershell -ExecutionPolicy Bypass -Command "$$s=Get-Date; $(1); $$e=Get-Date; $$t=$$e-$$s; $$h=[Math]::Floor($$t.TotalHours); $$m=$$t.Minutes; $$sec=$$t.Seconds; $$ts=[int]$$t.TotalSeconds; Write-Host (\"`nElapsed: {0:D2}:{1:D2}:{2:D2} ({3}s)\" -f [int]$$h, [int]$$m, [int]$$sec, $$ts)"
else
UNAME_S := $(shell uname -s)
# For Unix, we use a shell-based timing to ensure consistent format across BSD/macOS and GNU/Linux
TIME_CMD = START=$$(date +%s); $(1); END=$$(date +%s); DIFF=$$(($$END - $$START)); H=$$(($$DIFF / 3600)); M=$$(($$DIFF % 3600 / 60)); S=$$(($$DIFF % 60)); printf "\nElapsed: %02d:%02d:%02d (%ds)\n" $$H $$M $$S $$DIFF
ifeq ($(UNAME_S),Darwin)
BUNDLE = app,dmg
else
BUNDLE = app
endif
endif

# Helper to run command with timing
# Usage: $(call run_timed,command)
run_timed = @$(subst $$(1),$(1),$(TIME_CMD))

build: clean
	$(call run_timed,npm run tauri -- build --bundles $(BUNDLE))

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
	$(call run_timed,python scripts/upload.py)
