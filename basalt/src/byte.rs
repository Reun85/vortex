use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Byte(usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bit(usize);

impl Byte {
    fn as_bits(self) -> Bit {
        self.into()
    }
}

impl fmt::Display for Bit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            1 => write!(f, "1 bit"),
            x => {
                write!(f, "{x} bits")
            }
        }
    }
}
impl fmt::Display for Byte {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            1 => write!(f, "1 byte"),
            x => {
                write!(f, "{x} bytes")
            }
        }
    }
}

const fn size_of<T>() -> Byte {
    Byte(std::mem::size_of::<T>())
}

#[derive(Debug)]
pub struct NotEnoughBitsError {
    pub bits: Bit,
}

impl std::error::Error for NotEnoughBitsError {}
impl fmt::Display for NotEnoughBitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Not enough bits to convert to a byte. Bits: {}",
            self.bits.0
        )
    }
}

impl TryFrom<Bit> for Byte {
    type Error = NotEnoughBitsError;

    fn try_from(value: Bit) -> Result<Self, Self::Error> {
        let div = value.0 / 8;
        if div * 8 == value.0 {
            Ok(Self(value.0 / 8))
        } else {
            Err(Self::Error { bits: value })
        }
    }
}

impl From<Byte> for Bit {
    fn from(value: Byte) -> Self {
        Self(value.0 * 8)
    }
}
