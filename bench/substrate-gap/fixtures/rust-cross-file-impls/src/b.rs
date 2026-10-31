use crate::a::Sq;
use crate::Shape;

impl Shape for Sq {
    fn area(&self) -> f64 {
        1.0
    }
}

impl<T: Clone> Shape for Vec<T> {
    fn area(&self) -> f64 {
        0.0
    }
}
