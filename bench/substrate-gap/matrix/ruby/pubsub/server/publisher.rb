require "google/cloud/pubsub"

pubsub = Google::Cloud::Pubsub.new
topic = pubsub.topic("orders")
topic.publish("order-1")
