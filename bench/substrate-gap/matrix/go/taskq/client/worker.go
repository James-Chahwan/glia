package main

import (
	"context"

	"github.com/hibiken/asynq"
)

func HandleWelcomeEmail(ctx context.Context, t *asynq.Task) error {
	return nil
}

func Serve(srv *asynq.Server) error {
	mux := asynq.NewServeMux()
	mux.HandleFunc("email:welcome", HandleWelcomeEmail)
	return srv.Run(mux)
}
