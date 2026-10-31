FROM python:3.12-slim
WORKDIR /opt/tool
COPY . .
RUN pip install --no-cache-dir .
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["mytool", "serve", "--port", "8080"]
HEALTHCHECK CMD ["curl", "-f", "http://localhost:8080/"]
