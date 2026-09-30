use apalis::prelude::*;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct WelcomeEmail {
    pub to: String,
}

pub async fn send_welcome(job: WelcomeEmail) -> Result<(), Error> {
    Ok(())
}

pub fn worker(storage: apalis_redis::RedisStorage<WelcomeEmail>) -> Worker<()> {
    WorkerBuilder::new("welcome-email").backend(storage).build_fn(send_welcome)
}
