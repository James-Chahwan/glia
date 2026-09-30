use aws_sdk_secretsmanager::Client;

pub async fn db_password(client: &Client) -> Option<String> {
    let out = client.get_secret_value().secret_id("prod/db-password").send().await.ok()?;
    out.secret_string().map(str::to_string)
}
