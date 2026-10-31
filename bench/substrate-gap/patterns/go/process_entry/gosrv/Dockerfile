FROM golang:1.22 AS build
WORKDIR /src
COPY . .
RUN go build -o /bin/server ./cmd/server

FROM gcr.io/distroless/static
COPY --from=build /bin/server /bin/server
ENTRYPOINT ["/bin/server"]
