FROM rust:1.80 AS build
WORKDIR /app
COPY . .
RUN cargo build --release --bin api

FROM debian:bookworm-slim
COPY --from=build /app/target/release/api /usr/local/bin/api
CMD ["api"]
