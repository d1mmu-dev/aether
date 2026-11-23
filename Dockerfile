# syntax=docker/dockerfile:1

# Only linux/amd64 has an approved, checksum-pinned ORT package.
FROM debian:bookworm-slim AS ort-runtime
RUN apt-get update && apt-get install -y --no-install-recommends curl python3 ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY scripts/acquire-ort.sh scripts/provision-ort.py /scripts/
RUN mkdir -p /opt/aether && sh /scripts/acquire-ort.sh /opt/aether/ort

FROM debian:bookworm-slim AS soxr-runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc libc6-dev make cmake patch curl ca-certificates xz-utils binutils \
 && rm -rf /var/lib/apt/lists/*
COPY scripts/build-soxr.sh /src/scripts/build-soxr.sh
COPY native/soxr /src/native/soxr
RUN sh /src/scripts/build-soxr.sh /opt/aether/soxr

# Toolchain for `vertigo build`: vertigo-cli (same version as the `vertigo` crate, =0.13.4 in
# crates/aether-web/Cargo.toml) and wasm-opt from a binaryen release (bookworm's apt binaryen is too old).
FROM rust:1-bookworm AS web-toolchain
ARG BINARYEN_VERSION=133
RUN rustup target add wasm32-unknown-unknown \
 && cargo install vertigo-cli --version 0.13.4 --locked \
 && case "$(uname -m)" in x86_64) arch=x86_64 ;; aarch64) arch=aarch64 ;; *) echo "unsupported arch" >&2; exit 1 ;; esac \
 && curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/binaryen-version_${BINARYEN_VERSION}-${arch}-linux.tar.gz" \
    | tar xz -C /opt \
 && ln -s "/opt/binaryen-version_${BINARYEN_VERSION}/bin/wasm-opt" /usr/local/bin/wasm-opt \
 && wasm-opt --version

# aether-web build. The output directory is the SAME path at build time and at run time: vertigo serves the
# directory under the path it is given, so the public asset URLs contain it (crates/aether-web/README.md).
FROM web-toolchain AS web
WORKDIR /src
COPY . .
RUN vertigo build aether-web --dest-dir /srv/aether/web \
 && ls -l /srv/aether/web

FROM rust:1-bookworm AS backend
# cmake: opusic-sys builds the vendored libopus (Opus decoding, D-251) and links it statically.
RUN apt-get update && apt-get install -y --no-install-recommends cmake \
 && rm -rf /var/lib/apt/lists/*
COPY --from=soxr-runtime /opt/aether/soxr /opt/aether/soxr
ENV LIBRARY_PATH=/opt/aether/soxr/lib
ENV LD_LIBRARY_PATH=/opt/aether/soxr/lib
WORKDIR /src
COPY . .
ENV SQLX_OFFLINE=true
RUN cargo build --release -p aether-server
# libopus is BSD-3-Clause: its copyright notice travels with the binary (native/opus/README.md).
RUN set -e; copying="$(find /usr/local/cargo/registry/src -path '*/opusic-sys-*/opus/COPYING')"; \
    test "$(printf '%s\n' "$copying" | wc -l)" = 1; \
    install -D -m 0644 "$copying" /opt/aether/notices/libopus/COPYING; \
    install -D -m 0644 native/opus/README.md /opt/aether/notices/libopus/README.md

# Postgres 17 + pgvector on Debian bookworm; adds the aether binary and the aether-web build.
FROM pgvector/pgvector:pg17 AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends tini ca-certificates ffmpeg libgomp1 libstdc++6 libgcc-s1 libssl3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home /data --shell /usr/sbin/nologin aether
COPY --from=ort-runtime /opt/aether/ort /opt/aether/ort
COPY --from=soxr-runtime /opt/aether/soxr /opt/aether/soxr
RUN chown -R root:root /opt/aether && chmod -R a-w /opt/aether/ort /opt/aether/soxr
ENV ORT_DYLIB_PATH=/opt/aether/ort/libonnxruntime.so.1.22.0
ENV LD_LIBRARY_PATH=/opt/aether/soxr/lib
COPY --from=backend /src/target/release/aether /usr/local/bin/aether
COPY --from=backend /opt/aether/notices /opt/aether/notices
COPY --from=web /srv/aether/web /srv/aether/web
COPY docker/entrypoint.sh /usr/local/bin/aether-entrypoint
RUN chmod +x /usr/local/bin/aether-entrypoint
ENV AETHER_DATA_DIR=/data \
    AETHER_WEB_BUILD_DIR=/srv/aether/web \
    MUSIC_PATH=/music \
    PORT=3030
EXPOSE 3030
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s \
  CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/${PORT:-3030} && printf 'GET /health HTTP/1.0\\r\\n\\r\\n' >&3 && grep -qE '^HTTP/1\\.[01] 200' <&3"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/aether-entrypoint"]
