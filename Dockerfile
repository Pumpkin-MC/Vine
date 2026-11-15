FROM ubuntu:26.04

ARG TARGETARCH
ARG VINE_TAG=nightly

RUN apt-get -o Acquire::Retries=3 update && \
    DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y --no-install-recommends curl ca-certificates netcat-openbsd && \
    case "${TARGETARCH}" in \
        "amd64") BIN_ARCH="X64" ;; \
        "arm64") BIN_ARCH="ARM64" ;; \
        *) echo "Unsupported architecture: ${TARGETARCH}" && exit 1 ;; \
    esac && \
    if [ "${VINE_TAG}" = "latest" ]; then \
        DOWNLOAD_URL="https://github.com/Pumpkin-MC/Vine/releases/latest/download/vine-${BIN_ARCH}-Linux"; \
    else \
        DOWNLOAD_URL="https://github.com/Pumpkin-MC/Vine/releases/download/${VINE_TAG}/vine-${BIN_ARCH}-Linux"; \
    fi && \
    curl -fsSL --retry 3 "${DOWNLOAD_URL}" -o /usr/local/bin/vine && \
    chmod +x /usr/local/bin/vine && \
    apt-get purge -y --auto-remove curl && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd -g 2613 vine && \
    useradd -u 2613 -g vine -m -d /vine vine && \
    chown -R vine:vine /vine

WORKDIR /vine
USER vine:vine

ENV RUST_BACKTRACE=1
EXPOSE 25565 19132 19132/udp

ENTRYPOINT [ "vine" ]

HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD nc -z 127.0.0.1 25565 || exit 1
