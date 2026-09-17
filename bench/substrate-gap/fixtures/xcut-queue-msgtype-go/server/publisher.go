package svc

import (
	"github.com/nats-io/nats.go"
	"google.golang.org/protobuf/proto"

	pb "example.com/demo/gen"
)

// PublishOrder marshals an OrderCreated event and publishes it on "orders".
func PublishOrder(nc *nats.Conn, id string) error {
	data, err := proto.Marshal(&pb.OrderCreated{Id: id})
	if err != nil {
		return err
	}
	return nc.Publish("orders", data)
}
