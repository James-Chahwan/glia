import os

import redis
from celery.schedules import crontab

DB_URL = os.getenv("DB_URL")
REDIS_URL = f"redis://{os.getenv('REDIS_HOST')}:6379/0"
cache = redis.Redis()

CELERY_BEAT_SCHEDULE = {
    "nightly-report": {
        "task": "app.tasks.nightly_report",
        "schedule": crontab(minute=0, hour=2),
    },
}
