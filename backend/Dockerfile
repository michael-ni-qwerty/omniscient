# syntax=docker/dockerfile:1
FROM rust:1.82-slim-bookworm AS chef
WORKDIR /app
RUN cargo install cargo-chef

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN apt-get update && apt-get install -y cmake libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
ARG BIN
RUN cargo build --release --bin ${BIN}

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
ARG BIN
COPY --from=builder /app/target/release/${BIN} /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/${BIN}"]
