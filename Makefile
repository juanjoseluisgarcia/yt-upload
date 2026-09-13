.PHONY: build release test fmt fmt-check lint clean run install check man

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

check: fmt-check lint test

run:
	cargo run --

install:
	cargo install --path .

clean:
	cargo clean

man:
	cargo run --example gen-man
