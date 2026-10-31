FROM python:3.12-slim
WORKDIR /app
COPY . .
HEALTHCHECK --interval=30s --timeout=3s CMD curl -f http://localhost:8000/ || exit 1
CMD gunicorn \
    --bind 0.0.0.0:8000 app:app
