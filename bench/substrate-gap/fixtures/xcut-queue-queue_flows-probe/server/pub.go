package main
// nats
func pub(nc conn, p []byte) { nc.publish("orders", p) }
