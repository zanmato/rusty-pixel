FROM debian:bookworm-slim
ARG TARGETARCH
ENV GI_TYPELIB_PATH=/usr/lib/girepository-1.0

RUN apt-get update && apt-get install -y --no-install-recommends \
    libvips42 \
    curl \
    dumb-init \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

COPY bin/${TARGETARCH}/rusty-pixel /app/rustypixel
RUN chmod +x /app/rustypixel

HEALTHCHECK --interval=30s --start-period=10s CMD curl --fail http://localhost:6101/healthz || exit 1

EXPOSE 6100 6101

WORKDIR /app

USER nobody
ENTRYPOINT ["/usr/bin/dumb-init", "--"]
CMD ["/app/rustypixel"]
