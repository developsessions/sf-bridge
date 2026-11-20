# syntax=docker/dockerfile:1
FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin sf-bridge

FROM debian:trixie-slim
# Stockfish from Debian (17+, as the bridge needs).
RUN apt-get update \
 && apt-get install -y --no-install-recommends stockfish \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home-dir /data --shell /usr/sbin/nologin sfbridge \
 && mkdir /data && chown sfbridge /data
COPY --from=build /src/target/release/sf-bridge /usr/local/bin/sf-bridge
ENV SF_PATH=/usr/games/stockfish \
    TOKEN_FILE=/data/token.txt \
    TCP_PORT=7892
USER sfbridge
WORKDIR /data
VOLUME /data
EXPOSE 7892
ENTRYPOINT ["/usr/local/bin/sf-bridge"]
