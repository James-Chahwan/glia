use redis::Commands;

pub fn notify(con: &mut redis::Connection) -> redis::RedisResult<()> {
    con.publish("orders", "order-1")
}
