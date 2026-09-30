package shop

import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient
import software.amazon.awssdk.services.secretsmanager.model.GetSecretValueRequest

object Secrets {
  def dbPassword(sm: SecretsManagerClient): String =
    sm.getSecretValue(GetSecretValueRequest.builder().secretId("prod/db-password").build()).secretString()
}
