package worker

import (
	"github.com/nats-io/nats.go"
	"google.golang.org/protobuf/proto"

	pb "example.com/demo/gen"
)

// SubscribeShipments decodes every "shipments" message as a ShipmentDispatched
// event, but the publisher marshals ShipmentCreated — the MISMATCH row.
func SubscribeShipments(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("shipments", func(m *nats.Msg) {
		evt := &pb.ShipmentDispatched{}
		_ = proto.Unmarshal(m.Data, evt)
	})
}
