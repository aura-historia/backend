pub trait Mergeable {
    fn merge(&mut self, other: Self);
}
