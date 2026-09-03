.PHONY: all build run clean help release test-scripts update

all: help

ifeq ($(OS),Windows_NT)
BUNDLE = nsis
# For Windows, we use powershell to time the commands.
# Use $(1) as a placeholder for the command to be executed.
# We use [int] to ensure integer division/truncation for seconds.
# Wrap the -Command argument in single quotes so MSYS/sh doesn't expand
# PowerShell variables like $h/$m/$sec before PowerShell receives them.
TIME_CMD = powershell -ExecutionPolicy Bypass -Command '$$s=Get-Date; $(1); $$e=Get-Date; $$t=$$e-$$s; $$h=[Math]::Floor($$t.TotalHours); $$m=$$t.Minutes; $$sec=$$t.Seconds; $$ts=[int]$$t.TotalSeconds; Write-Host ("`nElapsed: {0:D2}:{1:D2}:{2:D2} ({3}s)" -f [int]$$h, [int]$$m, [int]$$sec, $$ts)'
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

run:
	npm run tauri -- dev

clean:
	@python -c "import shutil; [shutil.rmtree(p, ignore_errors=True) for p in ('src-tauri/target', 'dist')]"

help:
	@echo "Makefile targets:"
	@echo "  all       - Build the application (default)"
	@echo "  build     - Build the application"
	@echo "  run       - Run AuraTerm in development mode"
	@echo "  clean     - Remove build artifacts"
	@echo "  update    - Update npm and cargo dependencies to the latest compatible versions"
	@echo "  release   - Sync the changelog + current version into the AuraXLabs site checkout"
	@echo "  test-scripts - Run the unit tests for the Python release scripts"
	@echo "  help      - Show this help message"

release:
	@echo "Syncing changelog + version pin to the AuraXLabs site checkout..."
	@python scripts/sync_site.py

# The release scripts write into a *different* repo (the AuraXLabs checkout);
# a silent failure there leaves the site on the previous version with nothing
# in AuraTerm going red, so they carry their own tests.
test-scripts:
	@python -m unittest discover -s scripts -p 'test_*.py'

# Bring dependencies up to the newest versions the manifests already allow.
# `npm install` comes first on purpose: it re-syncs node_modules with
# package-lock.json. A stale tree silently pairs an old plugin frontend with a
# new Rust plugin backend, and the mismatched IPC command then fails at runtime
# rather than at build time.
update:
	@echo "==> Updating frontend dependencies"
	npm install
	npm update
	@echo ""
	@echo "==> Updating Rust dependencies"
	cd src-tauri && cargo update
	@echo ""
	@echo "==> Behind latest, needs a manual manifest bump (breaking majors):"
	-@npm outdated
	-@cd src-tauri && cargo update --dry-run --verbose 2>&1 | grep "Unchanged"
	@echo ""
	@echo "Verify with: npm run build && cd src-tauri && cargo check"
