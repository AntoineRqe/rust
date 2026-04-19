/// Shared trait for accessing byte array IDs (like `OrderId`, `EntityId`, etc.) as slices or hex strings.
pub trait IdExt {
    fn as_slice(&self) -> &[u8];
    fn to_hex(&self) -> String;
}

impl<T> IdExt for T
where
    T: std::ops::Deref<Target = [u8; 20]>,
{
    fn as_slice(&self) -> &[u8] {
        &**self
    }

    fn to_hex(&self) -> String {
        self.iter().map(|b| format!("{:02x}", b)).collect()
    }
}