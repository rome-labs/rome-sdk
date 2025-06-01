FROM anzaxyz/agave:v2.1.7 as solana
FROM rust:1.82.0 as build

RUN apt-get update && apt-get install -y protobuf-compiler

COPY rome-evm /opt/rome-evm
COPY rome-relayer /opt/rome-relayer
COPY rome-sdk /opt/rome-sdk

WORKDIR /opt/rome-sdk
RUN cargo test
RUN RUSTFLAGS="-D warnings" cargo build --release
