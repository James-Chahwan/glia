package main

import (
	"github.com/launchdarkly/go-sdk-common/v3/ldcontext"
	ld "github.com/launchdarkly/go-server-sdk/v6"
)

func Checkout(client *ld.LDClient, userKey string) string {
	enabled, _ := client.BoolVariation("new-checkout", ldcontext.New(userKey), false)
	if enabled {
		return "new"
	}
	return "legacy"
}
