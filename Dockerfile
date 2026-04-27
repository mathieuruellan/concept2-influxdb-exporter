# syntax=docker/dockerfile:1
# Build stage
FROM rust:latest AS build

ENV DEBIAN_FRONTEND=noninteractive

# Install build tools for static linking.
RUN apt-get update && apt-get install -y build-essential

# Add default target for proc-macros.
RUN rustup target add x86_64-unknown-linux-gnu

# Force static linking of C runtime.
ENV RUSTFLAGS="-C target-feature=+crt-static"

RUN USER=root cargo new --bin workdir
WORKDIR /workdir

# Copy only Cargo.toml to leverage Docker cache layers.
COPY Cargo.toml ./

# Create dummy main.rs to cache dependencies.
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo build --release --target x86_64-unknown-linux-gnu && rm -rf src

# Copy actual source code.
COPY src ./src

# Build the actual application.
RUN cargo build --release --target x86_64-unknown-linux-gnu

RUN ldd /workdir/target/x86_64-unknown-linux-gnu/release/concept2-influxdb
RUN file /workdir/target/x86_64-unknown-linux-gnu/release/concept2-influxdb

# Runtime stage
FROM scratch

ARG SOURCE_URL=https://forgejo.breizhbiniou.fr/mathieu/concept2_influxdb

LABEL org.opencontainers.image.source="${SOURCE_URL}" \
     org.opencontainers.image.maintainer="Mathieu Ruellan <mathieu.ruellan@gmail.com>"

WORKDIR /

# Copy binary from builder.
COPY --from=build /workdir/target/x86_64-unknown-linux-gnu/release/concept2-influxdb /concept2-influxdb

USER 1000
ENV RUST_LOG=info

EXPOSE 8000

VOLUME ["/data"]
CMD ["/concept2-influxdb"]