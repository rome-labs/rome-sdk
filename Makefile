CONFIG_DIR := $(shell realpath ../rollup-config)

include ../rome-scripts/config/Makefile

build: keypairs variables
	cargo build --release
	cargo build --release --features "evm"

lint: keypairs variables
	cargo clippy --all-targets --all-features -- -D warnings

doc: keypairs variables
	cargo doc --no-deps --all-features --release

test: keypairs variables
	env EVM_PROGRAM_KEYPAIR_PATH=$(EVM_LOADER_FILE) \
		OPERATOR_KEYPAIR_PATH=$(SOL_KEYPAIR_FILE) \
		EVM_EMULATOR_ADDR=${EVM_EMULATOR_ADDR} \
		cargo test --all-targets --all-features --release
