.PHONY: all build lint test run fmt check

all: build lint test

build:
	cargo build

lint:
	cargo fmt --all
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test

run:
	cargo run --

fmt:
	cargo fmt --all

check:
	cargo check
