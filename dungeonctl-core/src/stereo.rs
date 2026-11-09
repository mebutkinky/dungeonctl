/// A pair of two values that represent an audio channel each.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StereoValue<T> {
    /// The value for Channel A.
    pub a: T,
    /// The value for Channel B.
    pub b: T,
}

impl<T> StereoValue<T> {
    /// Create a value pair where both values are the same.
    pub fn symmetric(v: T) -> Self
    where
        T: Clone,
    {
        Self { a: v.clone(), b: v }
    }
}

impl<T> From<(T, T)> for StereoValue<T> {
    fn from((a, b): (T, T)) -> Self {
        Self { a, b }
    }
}

impl<T> From<[T; 2]> for StereoValue<T> {
    fn from([a, b]: [T; 2]) -> Self {
        Self { a, b }
    }
}

impl<T: binrw::BinRead> binrw::BinRead for StereoValue<T>
where
    (T, T): binrw::BinRead,
{
    type Args<'a> = <(T, T) as binrw::BinRead>::Args<'a>;

    fn read_options<R: std::io::Read + std::io::Seek>(
        reader: &mut R,
        endian: binrw::Endian,
        args: Self::Args<'_>,
    ) -> binrw::BinResult<Self> {
        <(T, T) as binrw::BinRead>::read_options(reader, endian, args).map(Self::from)
    }
}

impl<T: Copy + binrw::BinWrite> binrw::BinWrite for StereoValue<T>
where
    (T, T): binrw::BinWrite,
{
    type Args<'a> = <(T, T) as binrw::BinWrite>::Args<'a>;

    fn write_options<W: std::io::Write + std::io::Seek>(
        &self,
        writer: &mut W,
        endian: binrw::Endian,
        args: Self::Args<'_>,
    ) -> binrw::BinResult<()> {
        (self.a, self.b).write_options(writer, endian, args)
    }
}
