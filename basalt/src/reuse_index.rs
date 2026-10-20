pub type Index = u32;
pub type Epoch = u32;
type RawIDInner = u64;

#[derive(Clone, Debug)]
pub struct ID {
    index: Index,
    epoch: Epoch,
}
impl ID {
    const fn zip(self) -> RawID {
        RawID(
            (self.index as RawIDInner)
                | ((self.epoch as RawIDInner)
                    << (std::mem::size_of::<Index>() * 8)),
        )
    }
}

#[derive(Clone, Debug)]
pub struct RawID(RawIDInner);
impl RawID {
    const fn unzip(self) -> ID {
        #![allow(clippy::cast_possible_truncation)]
        #![allow(clippy::cast_precision_loss)]
        ID {
            index: (self.0 & (Index::MAX as RawIDInner)) as Index,
            epoch: (self.0 >> (std::mem::size_of::<Index>() * 8)) as Epoch,
        }
    }
}

impl From<ID> for RawID {
    fn from(value: ID) -> Self {
        value.zip()
    }
}

impl From<RawID> for ID {
    fn from(value: RawID) -> Self {
        value.unzip()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    fn main() {
        assert_eq!(
            std::mem::size_of::<Index>() + std::mem::size_of::<Epoch>(),
            std::mem::size_of::<RawIDInner>()
        );
    }
}
