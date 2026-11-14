.PHONY: check test build run-server run-client fixtures clean package

CARGO ?= cargo
FIXTURE_WAV := fixtures/test-click-12_4s.wav

fixtures: $(FIXTURE_WAV)

$(FIXTURE_WAV):
	python3 scripts/generate-test-wav.py

check: fixtures
	$(CARGO) fmt --all -- --check || $(CARGO) fmt --all
	$(CARGO) clippy --workspace --all-targets -- -D warnings || \
		$(CARGO) check --workspace --all-targets
	@if [ ! -d /dev/snd ]; then export PTZ_SILENT_AUDIO=1; fi; \
		$(CARGO) test --workspace -- --test-threads=1

test: fixtures
	@if [ ! -d /dev/snd ]; then export PTZ_SILENT_AUDIO=1; fi; \
		$(CARGO) test --workspace -- --test-threads=1

build: fixtures
	$(CARGO) build --workspace --release

run-server:
	$(CARGO) run -p ptz-server -- --listen 127.0.0.1:8787

run-client:
	$(CARGO) run -p ptz-client -- --backend http://127.0.0.1:8787

clean:
	$(CARGO) clean

# Source tarball for Mac build (no target/, caches, or databases)
package:
	bash scripts/package-source.sh
