FROM rust:1-bookworm AS builder

WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/xenonite /usr/local/bin/xenonite
COPY --from=builder /app/target/release/build/zvec-*/out/zvec-bundled/lib/libzvec_c_api.so /usr/local/lib/

ENV LD_LIBRARY_PATH=/usr/local/lib

ENV XENONITE_PORT=8700 \
    XENONITE_DATA_DIR=/var/lib/xenonite \
    ROCKY_LLM_URL=http://host.docker.internal:7777/v1 \
    ROCKY_EMBED_URL=http://host.docker.internal:7778/v1

VOLUME ["/var/lib/xenonite"]
EXPOSE 8700

CMD ["xenonite"]
