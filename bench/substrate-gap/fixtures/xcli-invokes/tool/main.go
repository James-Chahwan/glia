package main

import "github.com/spf13/cobra"

var rootCmd = &cobra.Command{Use: "mytool"}
var migrateCmd = &cobra.Command{Use: "migrate"}

func main() { rootCmd.AddCommand(migrateCmd); rootCmd.Execute() }
