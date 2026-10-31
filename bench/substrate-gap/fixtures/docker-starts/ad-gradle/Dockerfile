FROM eclipse-temurin:21 AS builder
WORKDIR /app
COPY . .
RUN ./gradlew installDist --no-daemon

FROM eclipse-temurin:21-jre
WORKDIR /app
COPY --from=builder /app .
ENTRYPOINT ["/app/build/install/ad/bin/Ad"]
