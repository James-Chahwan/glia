from celery.schedules import crontab

CELERY_BEAT = {"nightly": {"task": "app.tasks.nightly", "schedule": crontab(minute=0, hour=2)}}

LAYOUT = {"schedule": (("a", "b"),)}
