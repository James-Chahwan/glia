use apalis::prelude::*;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Email {
    pub to: String,
}

pub struct Report {
    pub id: u32,
}

pub async fn enqueue(storage: &mut MemoryStorage<Email>) {
    let mut reports = Vec::new();
    reports.push(Report { id: 1 });
    let _ = storage.push(Email { to: "a@b.c".into() }).await;
}
