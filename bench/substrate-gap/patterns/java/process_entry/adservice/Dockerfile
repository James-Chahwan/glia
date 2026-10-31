FROM eclipse-temurin:21 AS builder
WORKDIR /app
COPY . .
RUN ./gradlew installDist

FROM eclipse-temurin:21-jre
WORKDIR /app
COPY --from=builder /app .
ENTRYPOINT ["/app/build/install/shop/bin/Ad"]
