# Both stages use the same Alpine release so the libvips headers the binary is
# built against match the runtime library. The version is also pinned with a
# fuzzy match so an unexpected bump fails the build loudly instead of drifting
# into production unnoticed.
ARG ALPINE_VERSION=3.24
ARG VIPS_VERSION=8.18
ARG CARGO_CHEF_VERSION=0.1.77

FROM rust:1.96-alpine${ALPINE_VERSION} AS chef
ARG VIPS_VERSION
ARG CARGO_CHEF_VERSION
WORKDIR /app
RUN apk add --update --no-cache build-base musl-dev openssl-dev pkgconf \
  "vips-dev~=${VIPS_VERSION}"
RUN cargo install cargo-chef --locked --version "${CARGO_CHEF_VERSION}"

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this layer is cached unless Cargo.toml/Cargo.lock change
RUN cargo chef cook --release --recipe-path recipe.json --locked

# Copy the actual source code
COPY Cargo.toml Cargo.lock ./
COPY src ./src/
# Build the application - only rebuilt when source changes
RUN RUSTFLAGS="-C target-feature=-crt-static $(pkg-config vips --libs)" cargo build --release --locked

FROM alpine:${ALPINE_VERSION}
ARG VIPS_VERSION
ENV GI_TYPELIB_PATH=/usr/lib/girepository-1.0

RUN apk add --update --no-cache curl dumb-init "vips~=${VIPS_VERSION}"

COPY --from=builder /app/target/release/rusty-pixel /app/rustypixel
RUN chmod +x /app/rustypixel

HEALTHCHECK --interval=30s --start-period=10s CMD curl --fail http://localhost:6101/healthz || exit 1

EXPOSE 6100 6101

WORKDIR /app

USER nobody
ENTRYPOINT ["/usr/bin/dumb-init", "--"]
CMD ["/app/rustypixel"]
