use tokio::sync::{broadcast, mpsc};

#[derive(Clone, Debug)]
pub enum Event {
    Shipped(u32),
}

#[derive(Debug)]
pub enum Job {
    Run(u32),
}

pub async fn wire(
    events: broadcast::Sender<Event>,
    jobs: mpsc::Sender<Job>,
    mut inbox: mpsc::Receiver<Job>,
) {
    let mut rx = events.subscribe();
    let _ = events.send(Event::Shipped(1));
    let _ = jobs.send(Job::Run(1)).await;
    while let Some(Job::Run(n)) = inbox.recv().await {
        println!("{n}");
    }
    if let Ok(Event::Shipped(id)) = rx.recv().await {
        println!("{id}");
    }
}
