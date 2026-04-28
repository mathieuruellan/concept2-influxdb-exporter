# syntax=docker/dockerfile:1
# Build stage
FROM rust:alpine AS build


# Install build tools for static linking.
RUN apk add --no-cache build-base musl-dev coreutils file musl-utils


# Force static linking of C runtime.
ENV RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static -C relocation-model=static -C link-self-contained=yes"
ENV CFLAGS="-static"
ENV CXXFLAGS="-static"

RUN USER=root cargo new --bin workdir
WORKDIR /workdir

# Copy only Cargo.toml to leverage Docker cache layers.
COPY Cargo.toml ./

# Create dummy main.rs to cache dependencies.
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo build --release --target=x86_64-unknown-linux-musl && rm -rf src

# Copy actual source code.
COPY src ./src
# Force Cargo to detect the source change (dummy build artifact has same mtime otherwise)
RUN touch src/main.rs

# Build the actual application.
RUN cargo build --release --target=x86_64-unknown-linux-musl

# Verify static linking - check for no dynamic dependencies
RUN apk add --no-cache binutils && \
    if readelf -d /workdir/target/x86_64-unknown-linux-musl/release/concept2-influxdb 2>/dev/null | grep -q 'NEEDED'; then \
        echo "Error: Binary has dynamic dependencies!"; false; \
    else \
        echo "Binary is statically linked (no NEEDED entries)"; \
    fi
RUN file /workdir/target/x86_64-unknown-linux-musl/release/concept2-influxdb

# Runtime stage
FROM scratch

ARG SOURCE_URL=https://forgejo.breizhbiniou.fr/mathieu/concept2_influxdb

LABEL org.opencontainers.image.source="${SOURCE_URL}" \
     org.opencontainers.image.maintainer="Mathieu Ruellan <mathieu.ruellan@gmail.com>"

WORKDIR /

# Copy binary from builder.
COPY --from=build /workdir/target/x86_64-unknown-linux-musl/release/concept2-influxdb /concept2-influxdb

USER 1000

ENV RUST_LOG=info

EXPOSE 8000

VOLUME ["/data"]
CMD ["/concept2-influxdb"]