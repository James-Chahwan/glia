#[tokio::main]
async fn main() {
    let pool = sqlx::PgPool::connect("postgres://localhost/app").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
}
