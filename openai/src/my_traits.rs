pub trait Output {
    type Data;
    fn write(&mut self, data: &Self::Data) -> Result<(), Box<dyn std::error::Error>>;
}

pub trait Input {
    type Output;
    fn parse(&mut self) -> Result<Self::Output, Box<dyn std::error::Error>>;
}

