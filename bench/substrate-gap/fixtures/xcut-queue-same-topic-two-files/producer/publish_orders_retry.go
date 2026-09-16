package main

import (
	"github.com/nats-io/nats.go"
)

// RepublishOrder publishes onto the SAME "orders" subject from a DIFFERENT
// file in the SAME repo. Both files mint the identical NodeId
// (GRAPH_TYPE, repo, QUEUE_PRODUCER, "queue_producer:orders"), so merge_parses
// folds them into one node and only the appended cells keep the second file
// visible — CodeNav.parent_of is a single pointer that merge_nav overwrites.
func RepublishOrder(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}
