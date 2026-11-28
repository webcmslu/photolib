# Phototek server, for a NAS (ASUSTOR, Synology, QNAP…) or any Linux box.
# Build from the app folder (the repository root):
#
#   docker build -f src-tauri/server/Dockerfile -t phototek-server .
#
# Run it with the photo folder and a data folder mounted (see docs/server.md):
#
#   docker run -d --name phototek -p 8080:8080 \
#     -v /volume1/Photos:/photos -v /volume1/Docker/phototek:/data \
#     -e PHOTOTEK_PASSWORD=… phototek-server

# 1. The interface, built once: the same files the desktop app shows.
FROM node:22-trixie-slim AS web
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts
COPY index.html vite.config.ts svelte.config.js tsconfig.json ./
COPY src ./src
COPY public ./public
COPY app-icon.svg ./
RUN npm run build

# 2. The server. libheif (HEIC photos from iPhones) must be 1.17 or newer:
#    Debian 13 "trixie" has it.
FROM rust:1-trixie AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends libheif-dev clang \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app/src-tauri
COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./
COPY src-tauri/core ./core
COPY src-tauri/server ./server
# A member of the workspace (the phone app's photo library): Cargo reads it
# to load the workspace, without building it.
COPY src-tauri/plugins/photos ./plugins/photos
# The desktop app is part of the workspace but not built here: only its
# manifest is needed, with an empty library.
COPY src-tauri/build.rs ./
RUN mkdir -p src && echo "pub fn run() {}" > src/lib.rs && echo "fn main() {}" > src/main.rs
RUN cargo build --release -p phototek-server

# 3. What runs: the server, the interface and libheif.
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends libheif1 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=server /app/src-tauri/target/release/phototek-server /usr/local/bin/phototek-server
COPY --from=web /app/dist /web
ENV PHOTOTEK_LIBRARY=/photos PHOTOTEK_DATA=/data PHOTOTEK_WEB=/web PHOTOTEK_PORT=8080
VOLUME ["/data"]
EXPOSE 8080
CMD ["phototek-server"]
