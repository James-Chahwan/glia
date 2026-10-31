FROM python:3.12-slim AS base
WORKDIR /app
COPY . /app
FROM base AS runtime
CMD ["gunicorn", "app:app"]
