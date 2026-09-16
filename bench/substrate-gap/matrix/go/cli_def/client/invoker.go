package main

import "os/exec"

func RunSync() error {
	return exec.Command("mytool", "sync").Run()
}
