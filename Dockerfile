FROM rust:1.87-alpine3.22 AS builder
WORKDIR /app

RUN apk add --update --no-cache vips vips-dev build-base musl-dev openssl-dev

# Build and cache the dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo fetch
RUN cargo build --release
RUN rm src/main.rs

# Copy the actual code files and build the application
COPY src ./src/
# Update the file date
RUN touch src/main.rs
RUN RUSTFLAGS="-C target-feature=-crt-static $(pkg-config vips --libs)" cargo build --release

FROM alpine:3.22
ENV GI_TYPELIB_PATH=/usr/lib/girepository-1.0

RUN apk add --update --no-cache vips curl dumb-init

COPY --from=builder /app/target/release/rusty-pixel /app/rustypixel

HEALTHCHECK --interval=30s --start-period=10s CMD curl --fail http://localhost:7003/healthz || exit 1

EXPOSE 7002

WORKDIR /app

USER nobody
ENTRYPOINT ["/usr/bin/dumb-init", "--"]
CMD ["/app/rustypixel"]
