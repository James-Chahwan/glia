resource "google_pubsub_topic" "orders" {
  name = "orders"
}

resource "google_pubsub_subscription" "orders_worker" {
  name  = "orders-worker"
  topic = google_pubsub_topic.orders.name
}
