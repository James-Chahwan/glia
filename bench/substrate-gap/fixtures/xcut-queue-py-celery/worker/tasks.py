"""Consumer side: the Celery worker that owns the task."""

from celery import shared_task


@shared_task
def send_email(address):
    """Deliver the welcome mail."""
    return address
