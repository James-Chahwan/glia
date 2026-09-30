package main

import (
	"fmt"

	"github.com/spf13/cobra"
)

var syncCmd = &cobra.Command{Use: "sync", Run: func(cmd *cobra.Command, args []string) { fmt.Println("sync") }}
var rootCmd = &cobra.Command{Use: "mytool"}

func init() {
	rootCmd.AddCommand(syncCmd)
}
