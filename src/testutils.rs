use crate::image::{Image, Pixel};

#[cfg(test)]
pub struct FakeImage {
    width: u32,
    height: u32,
}

#[cfg(test)]
impl Image for FakeImage {
    fn get_width(&self) -> u32 {
        self.width
    }

    fn get_height(&self) -> u32 {
        self.height
    }

    fn pixel(&self, x: u32, y: u32) -> Pixel {
        assert!(x < self.width);
        assert!(y < self.height);
        (y * self.width + x) as u8
    }
}

#[cfg(test)]
impl FakeImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
        }
    }
}