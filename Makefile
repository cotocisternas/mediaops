# Operator shortcuts. `make help` lists targets.
# Pass extra cargo flags with CARGO_FLAGS, extra CLI args with ARGS.
# Example: make test OFFLINE=1
#          make mediaops ARGS='status --json'

CARGO      ?= cargo
PKG_CLI    ?= mediaops
PKG_DAEMON ?= mediaopsd
CARGO_FLAGS ?= --locked

ifeq ($(OFFLINE),1)
CARGO_FLAGS += --offline
endif

.DEFAULT_GOAL := help

.PHONY: help fetch build release check test test-arch test-live coverage clippy fmt fmt-check \
	run mediaops daemon musl ci install clean

help: ## Show this list
	@awk 'BEGIN {FS = ":.*##"; printf "mediaops\n\nTargets:\n"} \
		/^[a-zA-Z0-9_-]+:.*?##/ { printf "  %-12s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@printf "\nVariables: CARGO_FLAGS='$(CARGO_FLAGS)'  OFFLINE=$(OFFLINE)  ARGS='$(ARGS)'\n"

fetch: ## Download crates (needed before OFFLINE=1)
	$(CARGO) fetch --locked

build: ## Build the workspace (debug, symbols on)
	$(CARGO) build --workspace $(CARGO_FLAGS)

release: ## Build the workspace (release, optimized)
	$(CARGO) build --release --workspace $(CARGO_FLAGS)

check: ## Typecheck without producing binaries
	$(CARGO) check --workspace --all-targets $(CARGO_FLAGS)

test: ## Default suite (no GPU, no seedbox, no live-box). OFFLINE=1 after fetch
	$(CARGO) test --workspace $(CARGO_FLAGS)

test-arch: ## AD-2 crate-graph and I/O-boundary tests
	$(CARGO) test -p mediaops-arch-tests $(CARGO_FLAGS)

test-live: ## Compile the live-box gate (does not run it; does not SSH/encode)
	$(CARGO) test -p $(PKG_CLI) --features live-box $(CARGO_FLAGS) --test live --no-run

coverage: ## Line/region coverage (needs cargo-llvm-cov + rustup component llvm-tools-preview)
	$(CARGO) llvm-cov --workspace $(CARGO_FLAGS) --summary-only --ignore-filename-regex='(/tests/|target/)' --fail-under-lines 87

clippy: ## Lint the workspace
	$(CARGO) clippy --workspace --all-targets $(CARGO_FLAGS)

fmt: ## rustfmt
	$(CARGO) fmt --all

fmt-check: ## rustfmt --check
	$(CARGO) fmt --all -- --check

run: mediaops ## Run the CLI (default: --help). Override with ARGS='…'

mediaops: ## cargo run -p mediaops -- $(ARGS)
	$(CARGO) run -p $(PKG_CLI) $(CARGO_FLAGS) -- $(if $(ARGS),$(ARGS),--help)

daemon: ## cargo run -p mediaopsd -- $(ARGS)
	$(CARGO) run -p $(PKG_DAEMON) $(CARGO_FLAGS) -- $(if $(ARGS),$(ARGS),--help)

MUSL_TARGET := x86_64-unknown-linux-musl
# musl C toolchain: musl-gcc when installed, else zig as a drop-in cross compiler.
# zig rejects cc-rs's `--target=<rust-triple>` spelling, so a tiny wrapper strips it,
# and rustc must not add its own self-contained CRT on top of zig's (`_start` twice).
ZIG_WRAP := $(CURDIR)/scripts/zig-musl-cc
ifeq ($(shell command -v musl-gcc 2>/dev/null),)
  ifneq ($(shell command -v zig 2>/dev/null),)
    MUSL_ENV := CC_x86_64_unknown_linux_musl=$(ZIG_WRAP) \
      CXX_x86_64_unknown_linux_musl=$(ZIG_WRAP) \
      AR_x86_64_unknown_linux_musl="$(CURDIR)/scripts/zig-musl-ar" \
      CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=$(ZIG_WRAP) \
      CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-self-contained=no -C target-feature=+crt-static"
  endif
endif

musl: ## Link musl-static mediaopsd (musl-gcc, or zig as the C toolchain). Not part of make test.
	$(MUSL_ENV) $(CARGO) build --release --target $(MUSL_TARGET) -p $(PKG_DAEMON) --bin $(PKG_DAEMON) $(CARGO_FLAGS)

ci: fetch ## Same sequence as .github/workflows/ci.yml
	$(CARGO) test --locked --offline --workspace
	$(MAKE) musl OFFLINE=1

install: ## Install mediaops and mediaopsd into ~/.cargo/bin
	$(CARGO) install --path bins/mediaops --locked --force
	$(CARGO) install --path bins/mediaopsd --locked --force

clean: ## cargo clean
	$(CARGO) clean
