# Headless CC Manager (ccm-server) with the web UI embedded.
#   docker build -t cc-manager .
#   docker run -p 8787:8787 -v ccm-data:/data cc-manager

FROM node:22-bookworm-slim AS ui
WORKDIR /src
RUN corepack enable
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY index.html vite.config.ts tsconfig.json tailwind.config.js postcss.config.js ./
COPY src ./src
RUN pnpm build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Workspace member manifests must exist even though only ccm-server builds.
COPY src-tauri ./src-tauri
COPY --from=ui /src/dist ./dist
RUN cargo build --release --locked -p ccm-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home /data --uid 10001 ccm \
 && mkdir -p /data && chown ccm /data
COPY --from=build /src/target/release/ccm-server /usr/local/bin/ccm-server
USER ccm
ENV CCM_DATA_DIR=/data \
    CCM_LISTEN=0.0.0.0:8787
VOLUME /data
EXPOSE 8787
ENTRYPOINT ["ccm-server"]
CMD ["serve"]
