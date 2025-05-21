use crate::error::{Error, StResult};

/// `ByteReader` trait provides easy way to read u8/u16/u32/u64 fields from a u8
/// slice. This eliminates the dependency on scroll crate.
#[allow(dead_code)]
pub(crate) trait ByteReader {
    fn read8(&self, index: usize) -> StResult<u8>;
    fn read16(&self, index: usize) -> StResult<u16>;
    fn read32(&self, index: usize) -> StResult<u32>;
    fn _read64(&self, index: usize) -> StResult<u64>;
    fn read8_with(&self, index: &mut usize) -> StResult<u8>;
    fn read16_with(&self, index: &mut usize) -> StResult<u16>;
    fn read32_with(&self, index: &mut usize) -> StResult<u32>;
    fn _read64_with(&self, index: &mut usize) -> StResult<u64>;
}

impl ByteReader for [u8] {
    fn read8(&self, index: usize) -> StResult<u8> {
        if index + 1 > self.len() {
            return Err(Error::BufferTooShort(index));
        }
        let slice = &self[index..index + 1];
        Ok(u8::from_le_bytes([slice[0]]))
    }

    fn read16(&self, index: usize) -> StResult<u16> {
        if index + 2 > self.len() {
            return Err(Error::BufferTooShort(index));
        }
        let slice = &self[index..index + 2];
        Ok(u16::from_le_bytes([slice[0], slice[1]]))
    }

    fn read32(&self, index: usize) -> StResult<u32> {
        if index + 4 > self.len() {
            return Err(Error::BufferTooShort(index));
        }
        let slice = &self[index..index + 4];
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn _read64(&self, index: usize) -> StResult<u64> {
        if index + 8 > self.len() {
            return Err(Error::BufferTooShort(index));
        }
        let slice = &self[index..index + 8];
        Ok(u64::from_le_bytes([slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7]]))
    }

    fn read8_with(&self, index: &mut usize) -> StResult<u8> {
        let res = self.read8(*index);
        if res.is_ok() {
            *index += core::mem::size_of::<u8>();
        }
        res
    }

    fn read16_with(&self, index: &mut usize) -> StResult<u16> {
        let res = self.read16(*index);
        if res.is_ok() {
            *index += core::mem::size_of::<u16>();
        }
        res
    }

    fn read32_with(&self, index: &mut usize) -> StResult<u32> {
        let res = self.read32(*index);
        if res.is_ok() {
            *index += core::mem::size_of::<u32>();
        }
        res
    }

    fn _read64_with(&self, index: &mut usize) -> StResult<u64> {
        let res = self._read64(*index);
        if res.is_ok() {
            *index += core::mem::size_of::<u64>();
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read8() {
        let buffer = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(buffer.read8(0).unwrap(), 0x12);
        assert_eq!(buffer.read8(3).unwrap(), 0x78);
        assert!(buffer.read8(4).is_err());
    }

    #[test]
    fn test_read16() {
        let buffer = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(buffer.read16(0).unwrap(), 0x3412);
        assert_eq!(buffer.read16(2).unwrap(), 0x7856);
        assert!(buffer.read16(3).is_err());
    }

    #[test]
    fn test_read32() {
        let buffer = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(buffer.read32(0).unwrap(), 0x78563412);
        assert!(buffer.read32(1).is_err());
    }

    #[test]
    fn test_read64() {
        let buffer = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(buffer._read64(0).unwrap(), 0x0807060504030201);
        assert!(buffer._read64(1).is_err());
    }

    #[test]
    fn test_read8_with() {
        let buffer = [0x12, 0x34, 0x56, 0x78];
        let mut index = 0;
        assert_eq!(buffer.read8_with(&mut index).unwrap(), 0x12);
        assert_eq!(index, 1);
        assert_eq!(buffer.read8_with(&mut index).unwrap(), 0x34);
        assert_eq!(index, 2);
    }

    #[test]
    fn test_read16_with() {
        let buffer = [0x12, 0x34, 0x56, 0x78];
        let mut index = 0;
        assert_eq!(buffer.read16_with(&mut index).unwrap(), 0x3412);
        assert_eq!(index, 2);
        assert_eq!(buffer.read16_with(&mut index).unwrap(), 0x7856);
        assert_eq!(index, 4);
        assert!(buffer.read16_with(&mut index).is_err());
    }

    #[test]
    fn test_read32_with() {
        let buffer = [0x12, 0x34, 0x56, 0x78];
        let mut index = 0;
        assert_eq!(buffer.read32_with(&mut index).unwrap(), 0x78563412);
        assert_eq!(index, 4);
        assert!(buffer.read32_with(&mut index).is_err());
    }

    #[test]
    fn test_read64_with() {
        let buffer = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let mut index = 0;
        assert_eq!(buffer._read64_with(&mut index).unwrap(), 0x0807060504030201);
        assert_eq!(index, 8);
        assert!(buffer._read64_with(&mut index).is_err());
    }

    #[test]
    fn test_error_buffer_too_short() {
        let buffer = [0x12, 0x34];
        assert_eq!(buffer.read8(2).unwrap_err(), Error::BufferTooShort(2));
        assert_eq!(buffer.read16(1).unwrap_err(), Error::BufferTooShort(1));
        assert_eq!(buffer.read32(0).unwrap_err(), Error::BufferTooShort(0));
        assert_eq!(buffer._read64(0).unwrap_err(), Error::BufferTooShort(0));
    }
}
