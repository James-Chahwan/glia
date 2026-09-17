from celery import Celery

celery = Celery("tasks", broker="redis://localhost:6379/0")


@celery.task
def process_order(order_id):
    return order_id
