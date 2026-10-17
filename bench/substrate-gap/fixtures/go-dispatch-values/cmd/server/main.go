package main

import (
	"example.com/shop/indexer"
	"example.com/shop/server"
	"example.com/shop/services"
)

func notify(id string) {}

func main() {
	svc := services.New()
	_ = svc.Upload("kyc/doc")
	_ = svc.Create("AUDD/PGK")
	_ = svc.Reasons("u1")
	_ = svc.Lookup("review")
	_ = (&services.Ledger{}).Pairs("AUDD/PGK")
	_ = services.Wrap()
	services.Pick()
	_ = (&services.Rules{}).Topics()
	_ = services.Topic{}.Describe()
	idx := indexer.New()
	idx.SetOnCompleted(svc.OnDone)
	notify := func(id string) {}
	idx.SetOnCompleted(notify)
	go idx.Run()
	_ = server.Setup().Run()
}
