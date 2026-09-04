//! A safe wrapper over a raw EFI Graphics Output Protocol interface pointer.
//!
//! Since Patina does not have a `Service`-based Graphics Output abstraction, this component interacts
//! with the protocol directly. Every `unsafe` FFI call the console makes into Graphics Output is localized
//! to this module.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::ptr::NonNull;

use patina::{
    error::{EfiError, Result},
    standard::efi::protocols::graphics_output,
};

pub(crate) use graphics_output::BltPixel;

/// Pixel width of one character cell, per the UEFI HII "narrow glyph" definition.
pub(crate) const GLYPH_WIDTH: usize = 8;
/// Pixel height of one character cell, per the UEFI HII "narrow glyph" definition.
pub(crate) const GLYPH_HEIGHT: usize = 19;

/// A single black pixel, used as a fallback color and for the initial full-screen clear on a
/// mode change.
pub(crate) const BLACK_PIXEL: BltPixel = BltPixel { blue: 0, green: 0, red: 0, reserved: 0 };

/// The 16 colors addressable through `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL.SetAttribute()`, indexed by
/// the low nibble (foreground) or high nibble (background) of the attribute byte.
pub(crate) const TEXT_COLORS: [BltPixel; 16] = [
    BltPixel { blue: 0x00, green: 0x00, red: 0x00, reserved: 0x00 }, // BLACK
    BltPixel { blue: 0x98, green: 0x00, red: 0x00, reserved: 0x00 }, // BLUE
    BltPixel { blue: 0x00, green: 0x98, red: 0x00, reserved: 0x00 }, // GREEN
    BltPixel { blue: 0x98, green: 0x98, red: 0x00, reserved: 0x00 }, // CYAN
    BltPixel { blue: 0x00, green: 0x00, red: 0x98, reserved: 0x00 }, // RED
    BltPixel { blue: 0x98, green: 0x00, red: 0x98, reserved: 0x00 }, // MAGENTA
    BltPixel { blue: 0x00, green: 0x98, red: 0x98, reserved: 0x00 }, // BROWN
    BltPixel { blue: 0x98, green: 0x98, red: 0x98, reserved: 0x00 }, // LIGHTGRAY
    BltPixel { blue: 0x30, green: 0x30, red: 0x30, reserved: 0x00 }, // DARKGRAY
    BltPixel { blue: 0xff, green: 0x00, red: 0x00, reserved: 0x00 }, // LIGHTBLUE
    BltPixel { blue: 0x00, green: 0xff, red: 0x00, reserved: 0x00 }, // LIGHTGREEN
    BltPixel { blue: 0xff, green: 0xff, red: 0x00, reserved: 0x00 }, // LIGHTCYAN
    BltPixel { blue: 0x00, green: 0x00, red: 0xff, reserved: 0x00 }, // LIGHTRED
    BltPixel { blue: 0xff, green: 0x00, red: 0xff, reserved: 0x00 }, // LIGHTMAGENTA
    BltPixel { blue: 0x00, green: 0xff, red: 0xff, reserved: 0x00 }, // YELLOW
    BltPixel { blue: 0xff, green: 0xff, red: 0xff, reserved: 0x00 }, // WHITE
];

/// The horizontal/vertical pixel resolution reported by a GOP mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModeResolution {
    pub(crate) horizontal: u32,
    pub(crate) vertical: u32,
}

/// A non-owning, safe wrapper over an open EFI Graphics Output Protocol interface.
///
/// The pointee is owned by whatever installed the protocol (real hardware/firmware, or a virtual
/// GOP produced by another driver). This wrapper only borrows it for the lifetime of the component's
/// `BY_DRIVER` open.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GopHandle(NonNull<graphics_output::Protocol>);

impl GopHandle {
    /// Wraps a raw, already-open Graphics Output interface pointer.
    ///
    /// This is unsafe because once the pointer is stored, it is used by internal methods
    /// throughout the handle's lifetime.
    ///
    /// # Safety
    ///
    /// `ptr` must point to an active EFI Graphics Output Protocol instance for as long as the
    /// returned handle (and any copies of it) are used.
    pub(crate) unsafe fn new(ptr: NonNull<graphics_output::Protocol>) -> Self {
        Self(ptr)
    }

    /// Returns the raw interface pointer, for handing to another protocol.
    pub(crate) fn interface(&self) -> NonNull<graphics_output::Protocol> {
        self.0
    }

    /// Returns the currently active mode number.
    pub(crate) fn current_mode_number(&self) -> u32 {
        // SAFETY: `self.0` is an active Graphics Output interface for the lifetime of this handle
        // (the caller's contract on `new`), and `mode` is always non-null once installed.
        unsafe { (*self.0.as_ref().mode).mode }
    }

    /// Returns the maximum mode number (exclusive) `query_mode`/`set_mode` accept.
    pub(crate) fn max_mode(&self) -> u32 {
        // SAFETY: as in `current_mode_number`.
        unsafe { (*self.0.as_ref().mode).max_mode }
    }

    /// Returns the resolution of the currently active mode.
    pub(crate) fn current_resolution(&self) -> Result<ModeResolution> {
        self.query_mode(self.current_mode_number())
    }

    /// Queries the resolution of `mode_number`.
    ///
    /// Per the UEFI specification, `QueryMode()` allocates its `Info` out-parameter from pool
    /// memory that the caller must free. Patina does not currently expose a pool-free capability
    /// to components, so this leaks one small `ModeInformation` allocation per call. This is
    /// considered acceptable as this is called only a handful of times per `Start()`.
    pub(crate) fn query_mode(&self, mode_number: u32) -> Result<ModeResolution> {
        let mut size_of_info = 0usize;
        let mut info: *mut graphics_output::ModeInformation = core::ptr::null_mut();

        // SAFETY: `self.0` is an active Graphics Output interface. `size_of_info`/`info` are valid,
        // properly aligned local out-parameters.
        let status =
            unsafe { (self.0.as_ref().query_mode)(self.0.as_ptr(), mode_number, &raw mut size_of_info, &raw mut info) };
        EfiError::status_to_result(status)?;

        let Some(info) = NonNull::new(info) else {
            return Err(EfiError::DeviceError);
        };
        // SAFETY: `query_mode` returned SUCCESS, so per spec `info` points to a valid, initialized
        // `ModeInformation` for the duration of this read.
        let info = unsafe { info.as_ref() };
        Ok(ModeResolution { horizontal: info.horizontal_resolution, vertical: info.vertical_resolution })
    }

    /// Sets the active mode to `mode_number`.
    pub(crate) fn set_mode(&mut self, mode_number: u32) -> Result<()> {
        // SAFETY: `self.0` is an active Graphics Output interface. `set_mode` has no other
        // preconditions beyond a valid mode number, which the caller is responsible for.
        let status = unsafe { (self.0.as_ref().set_mode)(self.0.as_ptr(), mode_number) };
        EfiError::status_to_result(status)
    }

    /// Fills a `width` x `height` rectangle at (`dest_x`, `dest_y`) with a solid `color`.
    pub(crate) fn blt_fill(
        &mut self,
        color: BltPixel,
        dest_x: usize,
        dest_y: usize,
        width: usize,
        height: usize,
    ) -> Result<()> {
        let mut color = color;
        // SAFETY: `self.0` is an active Graphics Output interface. `color` is a single in-bounds
        // pixel used as the fill source for `EfiBltVideoFill`, per the `Blt()` contract.
        let status = unsafe {
            (self.0.as_ref().blt)(
                self.0.as_ptr(),
                &raw mut color,
                graphics_output::BLT_VIDEO_FILL,
                0,
                0,
                dest_x,
                dest_y,
                width,
                height,
                0,
            )
        };
        EfiError::status_to_result(status)
    }

    /// Copies a `width` x `height` rectangle from (`src_x`, `src_y`) to (`dest_x`, `dest_y`)
    /// within the frame buffer, used to scroll the console up by one glyph row.
    pub(crate) fn blt_video_to_video(
        &mut self,
        src_x: usize,
        src_y: usize,
        dest_x: usize,
        dest_y: usize,
        width: usize,
        height: usize,
    ) -> Result<()> {
        // SAFETY: `self.0` is an active Graphics Output interface. The buffer pointer is null, which
        // is valid for `EfiBltVideoToVideo`.
        let status = unsafe {
            (self.0.as_ref().blt)(
                self.0.as_ptr(),
                core::ptr::null_mut(),
                graphics_output::BLT_VIDEO_TO_VIDEO,
                src_x,
                src_y,
                dest_x,
                dest_y,
                width,
                height,
                0,
            )
        };
        EfiError::status_to_result(status)
    }

    /// Reads back a `width` x `height` rectangle at (`src_x`, `src_y`) into `buffer`.
    ///
    /// `buffer` must hold exactly `width * height` pixels, tightly packed row-major.
    pub(crate) fn blt_video_to_buffer(
        &self,
        buffer: &mut [BltPixel],
        src_x: usize,
        src_y: usize,
        width: usize,
        height: usize,
    ) -> Result<()> {
        debug_assert!(buffer.len() >= width * height, "buffer too small for the requested rectangle");
        let delta = width * core::mem::size_of::<BltPixel>();
        // SAFETY: `self.0` is an active Graphics Output interface. `buffer` is valid for
        // `width * height` pixel writes, matching the `Delta` (row stride) passed below.
        let status = unsafe {
            (self.0.as_ref().blt)(
                self.0.as_ptr(),
                buffer.as_mut_ptr(),
                graphics_output::BLT_VIDEO_TO_BLT_BUFFER,
                src_x,
                src_y,
                0,
                0,
                width,
                height,
                delta,
            )
        };
        EfiError::status_to_result(status)
    }

    /// Writes a `width` x `height` rectangle from `buffer` to (`dest_x`, `dest_y`).
    ///
    /// `buffer` must hold exactly `width * height` pixels, tightly packed row-major.
    pub(crate) fn blt_buffer_to_video(
        &mut self,
        buffer: &[BltPixel],
        dest_x: usize,
        dest_y: usize,
        width: usize,
        height: usize,
    ) -> Result<()> {
        debug_assert!(buffer.len() >= width * height, "buffer too small for the requested rectangle");
        let delta = width * core::mem::size_of::<BltPixel>();
        // SAFETY: `self.0` is an active Graphics Output interface. `buffer` is valid for
        // `width * height` pixel reads, matching the `Delta` (row stride) passed below. `Blt()`
        // does not write through `BltBuffer` for `EfiBltBufferToVideo`, so the `*mut` cast of an
        // immutable buffer below does not create an active mutable alias.
        let status = unsafe {
            (self.0.as_ref().blt)(
                self.0.as_ptr(),
                buffer.as_ptr().cast_mut(),
                graphics_output::BLT_BUFFER_TO_VIDEO,
                0,
                0,
                dest_x,
                dest_y,
                width,
                height,
                delta,
            )
        };
        EfiError::status_to_result(status)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{FakeGop, same_pixel};

    #[test]
    fn test_gop_handle_interface_returns_same_pointer_as_construction() {
        let fake = FakeGop::new(&[(800, 600)]);
        let gop = fake.gop_handle();
        assert_eq!(gop.interface(), fake.handle());
    }

    #[test]
    fn test_gop_handle_current_mode_number_and_max_mode() {
        let fake = FakeGop::new(&[(800, 600), (1024, 768), (1920, 1080)]);
        let gop = fake.gop_handle();
        assert_eq!(gop.current_mode_number(), 0);
        assert_eq!(gop.max_mode(), 3);
    }

    #[test]
    fn test_gop_handle_current_resolution_matches_active_mode() {
        let fake = FakeGop::new(&[(800, 600), (1024, 768)]);
        let mut gop = fake.gop_handle();
        gop.set_mode(1).unwrap();
        let resolution = gop.current_resolution().unwrap();
        assert_eq!(resolution, ModeResolution { horizontal: 1024, vertical: 768 });
    }

    #[test]
    fn test_gop_handle_query_mode_returns_resolution() {
        let fake = FakeGop::new(&[(800, 600), (1024, 768)]);
        let gop = fake.gop_handle();
        let resolution = gop.query_mode(1).unwrap();
        assert_eq!(resolution, ModeResolution { horizontal: 1024, vertical: 768 });
    }

    #[test]
    fn test_gop_handle_query_mode_invalid_mode_number_errors() {
        let fake = FakeGop::new(&[(800, 600)]);
        let gop = fake.gop_handle();
        assert_eq!(gop.query_mode(1).unwrap_err(), EfiError::Unsupported);
    }

    #[test]
    fn test_gop_handle_query_mode_null_info_pointer_errors() {
        // A misbehaving GOP implementation that reports success but never writes `Info`.
        let fake = FakeGop::with_null_mode_info(&[(800, 600)]);
        let gop = fake.gop_handle();
        assert_eq!(gop.query_mode(0).unwrap_err(), EfiError::DeviceError);
    }

    #[test]
    fn test_gop_handle_set_mode_updates_current_mode_and_max_mode_unaffected() {
        let fake = FakeGop::new(&[(800, 600), (1024, 768)]);
        let mut gop = fake.gop_handle();
        gop.set_mode(1).unwrap();
        assert_eq!(gop.current_mode_number(), 1);
        assert_eq!(gop.max_mode(), 2);
    }

    #[test]
    fn test_gop_handle_set_mode_invalid_mode_number_errors() {
        let fake = FakeGop::new(&[(800, 600)]);
        let mut gop = fake.gop_handle();
        assert_eq!(gop.set_mode(5).unwrap_err(), EfiError::Unsupported);
    }

    #[test]
    fn test_gop_handle_set_mode_hardware_failure_errors() {
        let fake = FakeGop::with_unsettable_modes(&[(800, 600), (1024, 768)], &[1]);
        let mut gop = fake.gop_handle();
        assert_eq!(gop.set_mode(1).unwrap_err(), EfiError::DeviceError);
        // A failed SetMode leaves the previous mode active.
        assert_eq!(gop.current_mode_number(), 0);
    }

    #[test]
    fn test_gop_handle_blt_fill_paints_region() {
        let fake = FakeGop::new(&[(4, 4)]);
        let mut gop = fake.gop_handle();
        let red = BltPixel { blue: 0, green: 0, red: 0xFF, reserved: 0 };

        gop.blt_fill(red, 1, 1, 2, 2).unwrap();

        // Inside the filled rectangle.
        assert!(same_pixel(fake.pixel(1, 1), red));
        assert!(same_pixel(fake.pixel(2, 2), red));
        // Outside the filled rectangle, still the initial black.
        assert!(same_pixel(fake.pixel(0, 0), BLACK_PIXEL));
        assert!(same_pixel(fake.pixel(3, 3), BLACK_PIXEL));
    }

    #[test]
    fn test_gop_handle_blt_video_to_video_copies_region() {
        let fake = FakeGop::new(&[(4, 4)]);
        let mut gop = fake.gop_handle();
        let blue = BltPixel { blue: 0xFF, green: 0, red: 0, reserved: 0 };
        gop.blt_fill(blue, 0, 0, 1, 1).unwrap();

        gop.blt_video_to_video(0, 0, 2, 2, 1, 1).unwrap();

        assert!(same_pixel(fake.pixel(2, 2), blue));
        // The source pixel is untouched by a copy.
        assert!(same_pixel(fake.pixel(0, 0), blue));
    }

    #[test]
    fn test_gop_handle_blt_video_to_buffer_reads_region() {
        let fake = FakeGop::new(&[(4, 4)]);
        let mut gop = fake.gop_handle();
        let green = BltPixel { blue: 0, green: 0xFF, red: 0, reserved: 0 };
        gop.blt_fill(green, 1, 0, 2, 1).unwrap();

        let mut buffer = [BLACK_PIXEL; 2];
        gop.blt_video_to_buffer(&mut buffer, 1, 0, 2, 1).unwrap();

        assert!(same_pixel(buffer[0], green));
        assert!(same_pixel(buffer[1], green));
    }

    #[test]
    fn test_gop_handle_blt_buffer_to_video_writes_region() {
        let fake = FakeGop::new(&[(4, 4)]);
        let mut gop = fake.gop_handle();
        let white = BltPixel { blue: 0xFF, green: 0xFF, red: 0xFF, reserved: 0 };
        let buffer = [white; 4];

        gop.blt_buffer_to_video(&buffer, 0, 0, 2, 2).unwrap();

        assert!(same_pixel(fake.pixel(0, 0), white));
        assert!(same_pixel(fake.pixel(1, 1), white));
        // Outside the written rectangle, still the initial black.
        assert!(same_pixel(fake.pixel(3, 3), BLACK_PIXEL));
    }
}
