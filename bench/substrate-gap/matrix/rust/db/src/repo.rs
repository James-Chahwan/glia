use sqlx::PgPool;

pub async fn find_email(pool: &PgPool, id: i64) -> sqlx::Result<String> {
    let row: (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1").bind(id).fetch_one(pool).await?;
    Ok(row.0)
}
