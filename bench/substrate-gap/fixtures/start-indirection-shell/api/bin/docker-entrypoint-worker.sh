#!/bin/bash
set -e
python manage.py wait_for_db
# Run the worker
celery -A app worker -l info
