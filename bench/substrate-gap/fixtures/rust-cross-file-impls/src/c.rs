pub trait Label {
    fn label(&self) -> String;
}

impl Label for String {
    fn label(&self) -> String {
        self.clone()
    }
}

pub trait Scoped {
    fn scope(&self) -> u8;
}

impl<T: Scoped> Scoped for &T {
    fn scope(&self) -> u8 {
        (**self).scope()
    }
}
