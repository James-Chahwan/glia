package worker

import (
	"github.com/nats-io/nats.go"
	"google.golang.org/protobuf/proto"

	pb "example.com/demo/gen"
)

// SubscribeOrders decodes every "orders" message as an OrderCreated event —
// the same type the publisher marshals, so this pair is the MATCH row.
func SubscribeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders", func(m *nats.Msg) {
		evt := &pb.OrderCreated{}
		_ = proto.Unmarshal(m.Data, evt)
	})
}
