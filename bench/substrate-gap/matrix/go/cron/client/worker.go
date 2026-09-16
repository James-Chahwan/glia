package main

import (
	"log"

	"github.com/robfig/cron/v3"
)

func main() {
	c := cron.New()
	c.AddFunc("15 3 * * *", func() {
		log.Println("in-process sweep")
	})
	c.Start()
	select {}
}
