# syntax=docker/dockerfile:1
ARG GO_VERSION=1.22
FROM --platform=$BUILDPLATFORM golang:${GO_VERSION}-alpine AS builder
WORKDIR /src
COPY . .
RUN go build -o /frontend .

FROM builder AS tested
RUN go test ./...

FROM gcr.io/distroless/static
COPY --from=builder /frontend /frontend
ENTRYPOINT ["/frontend"]
