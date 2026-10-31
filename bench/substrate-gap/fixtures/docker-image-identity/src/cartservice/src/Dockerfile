FROM --platform=$BUILDPLATFORM mcr.microsoft.com/dotnet/sdk:8.0 AS builder
WORKDIR /app
COPY . .
RUN dotnet publish cartservice.csproj -o /cartservice

FROM mcr.microsoft.com/dotnet/runtime-deps:8.0
COPY --from=builder /cartservice /app
ENTRYPOINT ["/app/cartservice"]
