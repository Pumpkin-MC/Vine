FROM alpine:3.24

ARG TARGETARCH
ARG VINE_TAG=nightly

RUN apk add --no-cache curl ca-certificates && \
    case "${TARGETARCH}" in \
        "amd64") BIN_ARCH="X64" ;; \
        "arm64") BIN_ARCH="ARM64" ;; \
        *) echo "Unsupported architecture: ${TARGETARCH}" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/Pumpkin-MC/Vine/releases/download/${VINE_TAG}/vine-${BIN_ARCH}-Linux-musl" \
        -o /usr/local/bin/vine && \
    chmod +x /usr/local/bin/vine && \
    apk del curl

RUN addgroup -g 2613 vine && \
    adduser -u 2613 -G vine -D -h /vine vine && \
    chown -R vine:vine /vine

WORKDIR /vine
USER vine:vine

ENV RUST_BACKTRACE=1
EXPOSE 25565
EXPOSE 19132/udp

ENTRYPOINT [ "vine" ]

HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD nc -z 127.0.0.1 25565 || exit 1
