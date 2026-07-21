FROM rust:1.96-slim-bookworm AS build

RUN apt-get update && apt-get install -y \
    pkg-config libx11-dev libxext-dev libxft-dev \
    libxinerama-dev libxcursor-dev libxrandr-dev \
    libxi-dev libgl1-mesa-dev libegl1-mesa-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY native/ native/
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    libx11-6 libxext6 libxft4 libxinerama1 \
    libxcursor1 libxrandr2 libxi6 libgl1-mesa-glx \
    libegl1 && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/lumi-term /usr/local/bin/lumi-term

CMD ["lumi-term"]
