VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
export GITSYNC_VERSION := $(VERSION)

CARGO ?= cargo
ZIGBUILD ?= cargo zigbuild
PLATFORMS := linux-amd64 linux-arm64 linux-armv7 darwin-amd64 darwin-arm64 windows-amd64 windows-arm64

triple.linux-amd64 := x86_64-unknown-linux-musl
triple.linux-arm64 := aarch64-unknown-linux-musl
triple.linux-armv7 := armv7-unknown-linux-musleabihf
triple.darwin-amd64 := x86_64-apple-darwin
triple.darwin-arm64 := aarch64-apple-darwin
triple.windows-amd64 := x86_64-pc-windows-msvc
triple.windows-arm64 := aarch64-pc-windows-msvc

TRIPLE = $(triple.$(PLATFORM))
EXE = $(if $(filter windows-%,$(PLATFORM)),.exe)
BUILD = $(if $(filter linux-%,$(PLATFORM)),$(ZIGBUILD),$(CARGO) build)

.PHONY: build test release platform sums clean

build:
	$(CARGO) build --release --locked
	cp target/release/gitsync gitsync

test:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets --locked -- -D warnings
	$(CARGO) test --locked

release:
	rm -rf dist
	@for p in $(PLATFORMS); do $(MAKE) --no-print-directory platform PLATFORM=$$p || exit 1; done
	$(MAKE) --no-print-directory sums

platform:
	@test -n "$(TRIPLE)" || { echo "PLATFORM must be one of: $(PLATFORMS)" >&2; exit 2; }
	$(BUILD) --release --locked --target $(TRIPLE)
	mkdir -p dist
	cp target/$(TRIPLE)/release/gitsync$(EXE) dist/gitsync-$(PLATFORM)$(EXE)

sums:
	cd dist && sha256sum gitsync-* > SHA256SUMS

clean:
	$(CARGO) clean
	rm -rf dist gitsync
