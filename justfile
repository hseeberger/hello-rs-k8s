set shell := ["bash", "-uc"]

check:
	cargo check --tests

fmt:
	cargo +nightly fmt

fmt_check:
	cargo +nightly fmt --check

lint:
	cargo clippy --no-deps -- -D warnings

test:
	cargo test

fix:
	cargo fix --allow-dirty --allow-staged

all: fmt check lint test

run:
	RUST_LOG=hello_rs_k8s=debug,info \
		cargo run -p hello-rs-k8s \
		| jq
