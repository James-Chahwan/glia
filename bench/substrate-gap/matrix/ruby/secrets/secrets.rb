require "aws-sdk-secretsmanager"

def db_password
  client = Aws::SecretsManager::Client.new
  client.get_secret_value(secret_id: "prod/db-password").secret_string
end
