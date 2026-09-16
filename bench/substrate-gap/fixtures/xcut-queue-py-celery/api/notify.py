"""Producer side: an API handler that enqueues a Celery task."""

from celery import Celery

from worker.tasks import send_email

app = Celery("api")


def notify(user):
    # THE PHANTOM-TOPIC SHAPE: the first argument is the PAYLOAD (an address),
    # not a topic. The join key is the task `send_email`.
    send_email.delay("welcome@example.com")
    return {"queued": True}
