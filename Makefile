.PHONY: test check fmt run-server run-app probe install-mac test-install

DATA ?= $(HOME)/PTZProgrammer
PROBE_PORT ?= 18787

# Everything: unit tests, API tests against an in-process server, the crash test, the UI tests.
test:
	cargo test --workspace

# Formatting and lints, warnings as errors, then the install script on stand-in binaries.
check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	bash scripts/test-install.sh

fmt:
	cargo fmt --all

run-server:
	cargo run --release -p ptz-server --bin ptz-server -- --data "$(DATA)"

run-app:
	cargo run --release -p ptz-app

# Start a server on a throwaway data folder and try every transport action in every state.
probe:
	cargo build -p ptz-server --bins
	@dir=$$(mktemp -d); \
	target/debug/ptz-server --data "$$dir" --bind 127.0.0.1:$(PROBE_PORT) --no-audio >"$$dir/server.log" 2>&1 & pid=$$!; \
	sleep 1; \
	target/debug/ptz-probe --server http://127.0.0.1:$(PROBE_PORT); status=$$?; \
	kill $$pid 2>/dev/null; wait $$pid 2>/dev/null; rm -rf "$$dir"; \
	exit $$status

install-mac:
	bash scripts/macos-install.sh

test-install:
	bash scripts/test-install.sh
