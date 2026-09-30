use redis::Client;

pub fn listen(client: &Client) -> redis::RedisResult<()> {
    let mut con = client.get_connection()?;
    let mut pubsub = con.as_pubsub();
    pubsub.subscribe("orders")?;
    Ok(())
}
