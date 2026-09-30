package main

import "github.com/hibiken/asynq"

func Enqueue(client *asynq.Client, payload []byte) error {
	task := asynq.NewTask("email:welcome", payload)
	_, err := client.Enqueue(task)
	return err
}
