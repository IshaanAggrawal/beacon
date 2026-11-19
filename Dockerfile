# syntax=docker/dockerfile:1

# --- Build -------------------------------------------------------------------
FROM rust:1.100-slim-trixie AS build
WORKDIR /src

# Dependencies first, in their own layer, so changing beacon's code doesn't
# rebuild all of them.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src src
COPY ui ui
COPY data data
# Make sure cargo sees the real sources as newer than the placeholder build.
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# --- Run ---------------------------------------------------------------------
# glibc and libgcc only, no shell, runs as an unprivileged user. TLS roots for
# the optional LLM call are compiled into the binary.
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /src/target/release/beacon /usr/local/bin/beacon

# Hosting platforms usually set PORT themselves; this is the fallback.
ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/beacon"]
CMD ["serve"]
