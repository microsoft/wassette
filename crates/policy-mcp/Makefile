SHELL := bash

define HELP
Available targets:
  build        - Build in debug mode
  release      - Build in release mode
  clean        - Clean build artifacts
  test         - Run tests
  fmt          - Format code
  check        - Check code without building
  lint|clippy  - Run clippy linter
  help         - Show this help message
endef
export HELP

RELEASE-FILE := target/release/policy-mcp

default: build

build:
	cargo build

release: $(RELEASE-FILE)

$(RELEASE-FILE):
	cargo build --release

clean test fmt check clippy:
	cargo $@

lint:
	cargo clippy

help:
	@echo "$$HELP"