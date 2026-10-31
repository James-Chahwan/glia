# Multi-stage Go: the binary the ENTRYPOINT runs is the build line's output,
# chained through COPY --from.
FROM golang:1.22 AS build
WORKDIR /src
COPY . .
RUN CGO_ENABLED=0 go build \
    -ldflags="-s -w" \
    -o /bin/server ./cmd/server

FROM gcr.io/distroless/static
COPY --from=build /bin/server /bin/server
ENTRYPOINT ["/bin/server"]
