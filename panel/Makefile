GO ?= go
NPM ?= npm
BIN ?= bin/wirefox-panel
PROTO_DIR := ../proto
PROTO := wirefox/agent/v1/agent.proto
MODULE := github.com/ParsaKSH/WireFox/panel

.PHONY: all proto frontend build test

all: build

# Needs protoc, protoc-gen-go and protoc-gen-go-grpc on PATH.
proto:
	protoc -I $(PROTO_DIR) \
		--go_out=. --go_opt=module=$(MODULE) \
		--go-grpc_out=. --go-grpc_opt=module=$(MODULE) \
		$(PROTO)

frontend:
	cd frontend && $(NPM) ci && $(NPM) run build

build: frontend
	$(GO) build -trimpath -o $(BIN) ./cmd/wirefox-panel

test:
	$(GO) test ./...
