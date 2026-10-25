import random

from locust import FastHttpUser, TaskSet, between

products = ["0PUK6V6EV0", "1YMWWN1N4O"]


def index(l):
    l.client.get("/")


def view_cart(l):
    l.client.get("/cart")


def add_to_cart(l):
    l.client.post("/cart", {"product_id": random.choice(products), "quantity": 1})


class UserBehavior(TaskSet):
    tasks = {index: 1, view_cart: 2, add_to_cart: 2}


class WebsiteUser(FastHttpUser):
    tasks = [UserBehavior]
    wait_time = between(1, 10)
