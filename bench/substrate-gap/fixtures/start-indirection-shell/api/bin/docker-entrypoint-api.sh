#!/bin/bash
set -e
python manage.py wait_for_db
python -m app.seed

exec gunicorn -w "$GUNICORN_WORKERS" -k uvicorn.workers.UvicornWorker app.asgi:application --bind 0.0.0.0:8000
