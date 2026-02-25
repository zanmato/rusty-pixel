FROM rust:1.90-alpine3.22 AS chef
WORKDIR /app
RUN apk add --update --no-cache vips vips-dev build-base musl-dev openssl-dev
RUN cargo install cargo-chef

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

FROM alpine:3.22
ENV GI_TYPELIB_PATH=/usr/lib/girepository-1.0

RUN apk add --update --no-cache vips curl dumb-init

COPY --from=builder /app/target/release/rusty-pixel /app/rustypixel
RUN chmod +x /app/rustypixel

HEALTHCHECK --interval=30s --start-period=10s CMD curl --fail http://localhost:6101/healthz || exit 1

EXPOSE 6100 6101

WORKDIR /app

USER nobody
ENTRYPOINT ["/usr/bin/dumb-init", "--"]
CMD ["/app/rustypixel"]
