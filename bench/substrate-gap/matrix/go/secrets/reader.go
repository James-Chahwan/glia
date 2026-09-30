package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/secretsmanager"
)

func DBPassword(ctx context.Context, sm *secretsmanager.Client) (string, error) {
	out, err := sm.GetSecretValue(ctx, &secretsmanager.GetSecretValueInput{SecretId: aws.String("prod/db-password")})
	if err != nil {
		return "", err
	}
	return aws.ToString(out.SecretString), nil
}
