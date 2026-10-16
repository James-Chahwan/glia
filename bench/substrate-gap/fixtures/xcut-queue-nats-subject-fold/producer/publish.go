package producer

import (
	"fmt"

	"github.com/nats-io/nats.go"
)

// shipment names the subject one order's shipment events travel on.
func shipment(id string) string { return "shop.shipments." + id }

// PublishShipment announces that an order shipped.
func PublishShipment(conn *nats.Conn, id string) error {
	return conn.Publish(shipment(id), []byte(id))
}

// PublishPlaced announces a placed order on its own subject.
func PublishPlaced(nc *nats.Conn, id string) error {
	return nc.Publish(fmt.Sprintf("shop.orders.%s.placed", id), nil)
}

// PublishRegion publishes to a region's queue, the region glued into a token.
func PublishRegion(nc *nats.Conn, region string) error {
	return nc.Publish("orders-"+region, nil)
}
