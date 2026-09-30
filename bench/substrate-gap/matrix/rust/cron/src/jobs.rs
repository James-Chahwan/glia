use tokio_cron_scheduler::{Job, JobScheduler};

pub async fn start() {
    let sched = JobScheduler::new().await.unwrap();
    let job = Job::new_async("0 0 3 * * *", |_uuid, _l| Box::pin(async move { purge().await })).unwrap();
    sched.add(job).await.unwrap();
    sched.start().await.unwrap();
}

async fn purge() {}
