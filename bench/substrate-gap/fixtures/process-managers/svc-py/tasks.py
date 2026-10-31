from celery import Celery

celery = Celery("tasks")


@celery.task
def send_report(user_id):
    return user_id
