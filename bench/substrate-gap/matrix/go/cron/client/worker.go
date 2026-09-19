package main

import (
	"log"

	"github.com/robfig/cron/v3"
)

func sweep()   { log.Println("sweep") }
func cleanup() { log.Println("cleanup") }

func main() {
	c := cron.New()
	c.AddFunc("15 3 * * *", sweep)
	c.AddFunc("0 * * * *", cleanup)
	c.Start()
	select {}
}
