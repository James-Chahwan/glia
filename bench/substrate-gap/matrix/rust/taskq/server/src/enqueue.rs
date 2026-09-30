use apalis::prelude::*;

pub async fn enqueue(storage: &mut apalis_redis::RedisStorage<crate::WelcomeEmail>) {
    let _ = storage.push(crate::WelcomeEmail { to: "a@b.c".into() }).await;
}
