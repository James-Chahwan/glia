package producer

import (
	"fmt"
	"strings"

	"github.com/nats-io/nats.go"
)

// RefundSubject is the refund subject's format, one token per refund number.
const RefundSubject = "shop.refunds.%d"

// notifSubject is the subject one user's notifications travel on.
func notifSubject(user string) string {
	return "notif." + strings.TrimSpace(user)
}

// Notify pings one user.
func Notify(nc *nats.Conn, user string) error {
	return nc.Publish(notifSubject(user), nil)
}

// Refund announces refund n.
func Refund(nc *nats.Conn, n int) error {
	return nc.Publish(fmt.Sprintf(RefundSubject, n), nil)
}

// Audit records that an entity was created.
func Audit(nc *nats.Conn, id string) error {
	return nc.Publish("audit."+id+".created", nil)
}
