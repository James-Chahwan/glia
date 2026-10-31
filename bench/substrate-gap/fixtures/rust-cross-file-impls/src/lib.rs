pub mod a;
pub mod b;
pub mod c;

pub trait Shape {
    fn area(&self) -> f64;
    fn name(&self) -> &str {
        "shape"
    }
}
