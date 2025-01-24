FROM anzaxyz/agave:v2.1.7 as solana
FROM rust:1.79.0 as build

COPY rome-evm /opt/rome-evm
COPY rome-sdk /opt/rome-sdk

WORKDIR /opt/rome-sdk
RUN cargo test
RUN RUSTFLAGS="-D warnings" cargo build --release




