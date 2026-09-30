use async_graphql::{Object, SimpleObject};

#[derive(SimpleObject)]
pub struct Order {
    id: String,
}

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn orders(&self) -> Vec<Order> {
        vec![]
    }
}
