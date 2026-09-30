package com.example;

import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient;
import software.amazon.awssdk.services.secretsmanager.model.GetSecretValueRequest;

public class Secrets {
    public String dbPassword(SecretsManagerClient sm) {
        GetSecretValueRequest req = GetSecretValueRequest.builder().secretId("prod/db-password").build();
        return sm.getSecretValue(req).secretString();
    }
}
