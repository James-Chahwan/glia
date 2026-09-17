package svc

import (
	"github.com/nats-io/nats.go"
	"google.golang.org/protobuf/proto"

	pb "example.com/demo/gen"
)

// PublishShipment marshals a ShipmentCreated event and publishes it on "shipments".
func PublishShipment(nc *nats.Conn, orderID string) error {
	data, err := proto.Marshal(&pb.ShipmentCreated{OrderId: orderID})
	if err != nil {
		return err
	}
	return nc.Publish("shipments", data)
}
