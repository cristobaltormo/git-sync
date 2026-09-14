VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
LDFLAGS := -s -w -X main.version=$(VERSION)
TARGETS := linux/amd64 linux/arm64 linux/arm darwin/amd64 darwin/arm64

.PHONY: build test release clean

build:
	CGO_ENABLED=0 go build -trimpath -ldflags "$(LDFLAGS)" -o gitsync ./cmd/gitsync

test:
	go vet ./...
	go test -race -count=1 ./...

release:
	rm -rf dist && mkdir dist
	@for t in $(TARGETS); do \
		os=$${t%/*}; arch=$${t#*/}; name=gitsync-$$os-$$arch; \
		[ $$arch = arm ] && name=gitsync-$$os-armv7 && export GOARM=7; \
		echo $$name; \
		CGO_ENABLED=0 GOOS=$$os GOARCH=$$arch go build -trimpath -ldflags "$(LDFLAGS)" -o dist/$$name ./cmd/gitsync || exit 1; \
	done
	cd dist && sha256sum gitsync-* > SHA256SUMS

clean:
	rm -rf dist gitsync
