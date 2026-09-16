package main

import (
	"fmt"

	"github.com/spf13/cobra"
)

var syncCmd = &cobra.Command{Use: "sync", Short: "Sync records", Run: func(cmd *cobra.Command, args []string) {
	fmt.Println("syncing")
}}

var rootCmd = &cobra.Command{Use: "mytool", Short: "the fixture's binary"}

func init() {
	rootCmd.AddCommand(syncCmd)
}
