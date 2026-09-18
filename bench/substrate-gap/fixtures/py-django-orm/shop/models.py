from django.db import models


class Order(models.Model):
    total = models.IntegerField()

    class Meta:
        ordering = ["-total"]
