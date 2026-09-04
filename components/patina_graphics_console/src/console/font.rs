//! A safe wrapper over a raw EFI HII Font Protocol interface pointer.
//!
//! Patina doesn't provide a font/text-rendering service yet, so glyph rasterization goes out to the
//! installed EFI HII Font Protocol directly. Every `unsafe` FFI call the console makes into HII Font
//! is localized to this module.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::ptr::NonNull;

use patina::{
    error::{EfiError, Result},
    standard::efi::{
        self,
        protocols::{graphics_output, hii_font, hii_font_ex, hii_string},
    },
};

use super::gop::BltPixel;

/// A safe wrapper over an already-located EFI HII Font Protocol interface.
///
/// EFI HII Font Protocol is a system-wide service (located, never opened against a specific
/// controller), so unlike [`super::gop::GopHandle`] there is no per-controller open/close
/// lifecycle to manage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HiiFontHandle(NonNull<hii_font::Protocol>);

impl HiiFontHandle {
    /// Wraps a raw HII Font interface pointer.
    ///
    /// This is unsafe because once the pointer is stored, it is used by internal methods
    /// throughout the handle's lifetime.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a live EFI HII Font Protocol instance for as long as the returned
    /// handle (and any copies of it) are used.
    pub(crate) unsafe fn new(ptr: NonNull<hii_font::Protocol>) -> Self {
        Self(ptr)
    }

    /// Renders `text` directly onto `screen`'s frame buffer, using the system default font (no
    /// specific size/style/name requested).
    ///
    /// Returns `Ok(true)` if every character had a glyph, or `Ok(false)` if one or more characters
    /// had none and were skipped (matching `EFI_WARN_UNKNOWN_GLYPH`).
    pub(crate) fn draw_to_screen(&self, request: DrawToScreenRequest<'_>) -> Result<bool> {
        let display_info = hii_font_ex::DisplayInfo {
            foreground_color: request.foreground,
            background_color: request.background,
            // Zero mask: use the system default font rather than a specific size/style/name.
            font_info_mask: 0,
            font_info: hii_string::Info { font_style: 0, font_size: 0, font_name: [] },
        };

        // A pre-initialized, DIRECT_TO_SCREEN `ImageOutput` pointing at `screen`. The HII Font
        // Protocol specification states that this tells `StringToImage` to render in place rather
        // than allocate a bitmap.
        let mut image_output = hii_font_ex::ImageOutput {
            width: request.screen_width,
            height: request.screen_height,
            image: hii_font_ex::ImageOutputImage { screen: request.screen.as_ptr() },
        };
        let mut image_output_ptr: *mut hii_font_ex::ImageOutput = &raw mut image_output;

        let flags = hii_font::IGNORE_IF_NO_GLYPH | hii_font::DIRECT_TO_SCREEN | hii_font::IGNORE_LINE_BREAK;

        // SAFETY: `self.0` is an active HII Font interface (caller's contract on `new`). `text` is
        // NUL-terminated per this method's contract. `image_output_ptr` points at a fully
        // initialized, DIRECT_TO_SCREEN `ImageOutput`, so `StringToImage` writes through `screen`
        // instead of touching `*image_output_ptr`'s allocation. All other out-parameters are null,
        // which is valid since none of their results are needed here.
        let status = unsafe {
            (self.0.as_ref().string_to_image)(
                self.0.as_ptr(),
                flags,
                request.text.as_ptr().cast_mut(),
                &raw const display_info,
                &raw mut image_output_ptr,
                request.x,
                request.y,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };

        if status == efi::Status::WARN_UNKNOWN_GLYPH {
            return Ok(false);
        }
        EfiError::status_to_result(status)?;
        Ok(true)
    }

    /// Returns whether the system default font has a glyph for `ch`.
    pub(crate) fn has_glyph(&self, ch: u16) -> bool {
        let mut image_output: *mut hii_font_ex::ImageOutput = core::ptr::null_mut();
        // SAFETY: `self.0` is an active HII Font interface. `image_output` is a valid out-parameter.
        // Passing null display/cell-info requests the system default font.
        // On success this leaks the small bitmap `GetGlyph` allocates since Patina does not have
        // an explicit pool-free capability available to components. This is considered acceptable,
        // because the call only runs when a caller explicitly probes renderability via `TestString`,
        // not in the normal character-rendering path.
        let status = unsafe {
            (self.0.as_ref().get_glyph)(
                self.0.as_ptr(),
                ch,
                core::ptr::null(),
                &raw mut image_output,
                core::ptr::null_mut(),
            )
        };
        status == efi::Status::SUCCESS
    }
}

/// Parameters for [`HiiFontHandle::draw_to_screen`] are grouped into one struct because the
/// `StringToImage()` takes a lot of arguments.
pub(crate) struct DrawToScreenRequest<'a> {
    /// The screen to render onto.
    pub(crate) screen: NonNull<graphics_output::Protocol>,
    /// Pixel dimensions of the GOP mode currently active on `screen`.
    pub(crate) screen_width: u16,
    pub(crate) screen_height: u16,
    /// NUL-terminated UCS-2 text with no interior NUL, matching [`patina::string::Char16Str`]'s
    /// invariants.
    pub(crate) text: &'a [u16],
    /// Pixel position of the top-left corner to render at.
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) foreground: BltPixel,
    pub(crate) background: BltPixel,
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::char16;

    use crate::test_support::{FakeGop, FakeHiiFont};

    const WHITE: BltPixel = BltPixel { blue: 0xFF, green: 0xFF, red: 0xFF, reserved: 0 };
    const BLACK: BltPixel = BltPixel { blue: 0, green: 0, red: 0, reserved: 0 };

    /// `text` must outlive the request. Callers keep the NUL-terminated code units on their stack.
    fn request(screen: NonNull<graphics_output::Protocol>, text: &[u16]) -> DrawToScreenRequest<'_> {
        DrawToScreenRequest {
            screen,
            screen_width: 800,
            screen_height: 600,
            text,
            x: 3,
            y: 4,
            foreground: WHITE,
            background: BLACK,
        }
    }

    #[test]
    fn test_hii_font_handle_draw_to_screen_success_records_call() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::new();
        let handle = hii_font.hii_font_handle();

        let result = handle.draw_to_screen(request(gop.handle(), char16!("Hi").as_units_with_nul()));

        assert!(result.unwrap());
        let calls = hii_font.draw_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].text, char16!("Hi").as_units());
        assert_eq!((calls[0].x, calls[0].y), (3, 4));
    }

    #[test]
    fn test_hii_font_handle_draw_to_screen_unknown_glyph_returns_ok_false() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::with_unknown_glyphs(char16!("?").as_units());
        let handle = hii_font.hii_font_handle();

        let result = handle.draw_to_screen(request(gop.handle(), char16!("?").as_units_with_nul()));

        assert!(!result.unwrap());
    }

    #[test]
    fn test_hii_font_handle_draw_to_screen_propagates_hardware_error() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::failing(efi::Status::DEVICE_ERROR);
        let handle = hii_font.hii_font_handle();

        let result = handle.draw_to_screen(request(gop.handle(), char16!("A").as_units_with_nul()));

        assert_eq!(result.unwrap_err(), EfiError::DeviceError);
    }

    #[test]
    fn test_hii_font_handle_has_glyph_true_for_known_char() {
        let hii_font = FakeHiiFont::new();
        assert!(hii_font.hii_font_handle().has_glyph(u16::from(b'A')));
    }

    #[test]
    fn test_hii_font_handle_has_glyph_false_for_unknown_char() {
        let hii_font = FakeHiiFont::with_unknown_glyphs(char16!("Z").as_units());
        assert!(!hii_font.hii_font_handle().has_glyph(u16::from(b'Z')));
    }
}
