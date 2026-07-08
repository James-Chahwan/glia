package main
// nats
func sub(nc conn) { nc.subscribe("orders", nil) }
