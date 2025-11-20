use crate::error::{Error, StResult};

/// The `ByteReader` trait provides an easy way to read u8/u16/u32/u64 fields from
/// a u8 slice. This eliminates the dependency on the `scroll` crate.
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
            return Err(Error::OutOfBoundsRead { module: None, index });
        }
        let slice = &self[index..index + 1];
        Ok(u8::from_le_bytes([slice[0]]))
    }

    fn read16(&self, index: usize) -> StResult<u16> {
        if index + 2 > self.len() {
            return Err(Error::OutOfBoundsRead { module: None, index });
        }
        let slice = &self[index..index + 2];
        Ok(u16::from_le_bytes([slice[0], slice[1]]))
    }

    fn read32(&self, index: usize) -> StResult<u32> {
        if index + 4 > self.len() {
            return Err(Error::OutOfBoundsRead { module: None, index });
        }
        let slice = &self[index..index + 4];
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn _read64(&self, index: usize) -> StResult<u64> {
        if index + 8 > self.len() {
            return Err(Error::OutOfBoundsRead { module: None, index });
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

// SAFETY: The caller must ensure `pointer` remains a valid, properly aligned
// pointer to readable 8 bytes for the duration of this read.
pub(crate) unsafe fn read_pointer64(pointer: u64) -> StResult<u64> {
    if pointer == 0 {
        return Err(Error::OutOfBoundsRead { module: None, index: 0 });
    }

    // SAFETY: The caller is expected to uphold the calling safety requirements
    // for `pointer`.
    Ok(unsafe { *(pointer as *const u64) })
}

#[cfg(test)]
#[coverage(off)]
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
    fn test_error_out_of_bounds_read() {
        let buffer = [0x12, 0x34];
        assert_eq!(buffer.read8(2).unwrap_err(), Error::OutOfBoundsRead { module: None, index: 2 });
        assert_eq!(buffer.read16(1).unwrap_err(), Error::OutOfBoundsRead { module: None, index: 1 });
        assert_eq!(buffer.read32(0).unwrap_err(), Error::OutOfBoundsRead { module: None, index: 0 });
        assert_eq!(buffer._read64(0).unwrap_err(), Error::OutOfBoundsRead { module: None, index: 0 });
    }

    #[test]
    fn read_pointer64_reads_value() {
        let value: u64 = 0x0123_4567_89AB_CDEF;
        let ptr = &value as *const u64 as u64;
        // SAFETY: Test creates a valid pointer from a stack variable that remains live for the duration of this call.
        assert_eq!(unsafe { read_pointer64(ptr).unwrap() }, value);
    }

    #[test]
    fn read_pointer64_supports_pointer_arithmetic() {
        let values = [0xAABB_CCDD_EEFF_0011u64, 0x2233_4455_6677_8899u64];
        let base = values.as_ptr() as u64;
        // SAFETY: Test creates valid pointers from a stack array that remains live for the duration of these calls.
        assert_eq!(unsafe { read_pointer64(base).unwrap() }, values[0]);
        // SAFETY: Same as above; pointer arithmetic stays within the array bounds.
        assert_eq!(unsafe { read_pointer64(base + core::mem::size_of::<u64>() as u64).unwrap() }, values[1]);
    }

    #[test]
    fn read_pointer64_rejects_null_pointer() {
        // SAFETY: Test intentionally passes a null pointer to verify error handling.
        assert_eq!(unsafe { read_pointer64(0) }.unwrap_err(), Error::OutOfBoundsRead { module: None, index: 0 });
    }
}
