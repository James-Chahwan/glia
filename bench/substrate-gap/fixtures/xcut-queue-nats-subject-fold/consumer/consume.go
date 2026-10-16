package consumer

import (
	"github.com/nats-io/nats.go"
)

// Start subscribes the shop's handlers.
func Start(nc *nats.Conn) {
	nc.Subscribe("shop.shipments.*", onShipment)
	nc.Subscribe("shop.orders.>", onOrder)
	nc.Subscribe("shop.refunds.*", onRefund)
	nc.Subscribe("audit.*.created", onAudit)
	nc.Subscribe("notif.alice", onAlice)
	nc.Subscribe("orders-eu", onRegion)
}

func onShipment(m *nats.Msg) {}

func onOrder(m *nats.Msg) {}

func onRefund(m *nats.Msg) {}

func onAudit(m *nats.Msg) {}

func onAlice(m *nats.Msg) {}

func onRegion(m *nats.Msg) {}
