use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub enum Event {
    OrderPlaced(String),
}

pub async fn wire() {
    let (tx, mut rx) = broadcast::channel::<Event>(16);
    tokio::spawn(async move {
        while let Ok(Event::OrderPlaced(id)) = rx.recv().await {
            println!("{id}");
        }
    });
    let _ = tx.send(Event::OrderPlaced("o-1".into()));
}
