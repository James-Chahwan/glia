require "google/cloud/pubsub"

pubsub = Google::Cloud::Pubsub.new
sub = pubsub.subscription("orders")
listener = sub.listen { |msg| msg.acknowledge! }
listener.start
