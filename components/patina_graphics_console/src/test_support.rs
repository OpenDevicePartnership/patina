//! Shared test doubles for the raw EFI protocols this crate consumes.
//!
//! [`FakeGop`] and [`FakeHiiFont`] back a real (if behaviorally simplified) `Protocol` struct with
//! actual function pointers, so code under test dereferences genuine, correctly laid out protocol
//! interfaces exactly as it would in production. Both are leaked to `'static` (same tradeoff
//! [`Service::mock`] already documents) so tests can hand out [`GopHandle`]/[`HiiFontHandle`]
//! values without fighting self-referential lifetimes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use alloc::{boxed::Box, vec::Vec};
use core::cell::RefCell;
use core::ptr::NonNull;

use patina::{
    component::service::{
        Service,
        compat_memory::{CompatMemoryManager, MockCompatMemoryManager},
        uefi_services::tpl::{MockTplServices, PreviousTpl, TplServices},
    },
    standard::efi::{
        self,
        protocols::{graphics_output, hii_font, hii_font_ex},
    },
};

use crate::console::{font::HiiFontHandle, gop::GopHandle};

/// Builds a [`Service<dyn TplServices>`] that raises/restores TPL any number of times without
/// asserting a specific count. Reused by every test that locks a [`patina::uefi::tpl_mutex::TplMutex`]
/// (directly or through [`super::console::output::SimpleTextOutputHolder`]) so each test does not
/// have to predict how many times a given call locks internally.
pub(crate) fn permissive_tpl() -> Service<dyn TplServices> {
    let mut mock = MockTplServices::new();
    mock.expect_raise_tpl().returning(|_| PreviousTpl::from_raw(0));
    mock.expect_restore_tpl().returning(|_| {});
    Service::mock(Box::new(mock))
}

/// Builds a [`Service<dyn CompatMemoryManager>`] that frees any number of pool allocations without
/// asserting a specific count.
pub(crate) fn permissive_compat_memory_manager() -> Service<dyn CompatMemoryManager> {
    let mut mock = MockCompatMemoryManager::new();
    mock.expect_free_pool().returning(|_| Ok(()));
    Service::mock(Box::new(mock))
}

/// Compares two [`graphics_output::BltPixel`] values field by field, since the upstream type does
/// not derive `PartialEq`.
pub(crate) fn same_pixel(a: graphics_output::BltPixel, b: graphics_output::BltPixel) -> bool {
    a.blue == b.blue && a.green == b.green && a.red == b.red && a.reserved == b.reserved
}

/// Mutable state behind a [`FakeGop`], borrowed by its `extern "efiapi"` fake methods.
struct FakeGopState {
    mode_infos: Vec<graphics_output::ModeInformation>,
    unsettable: Vec<bool>,
    framebuffer: Vec<graphics_output::BltPixel>,
    set_mode_calls: usize,
    /// When set, `QueryMode()` reports success but writes a null `Info` pointer, simulating a
    /// misbehaving GOP implementation.
    report_null_mode_info: bool,
}

/// A fake Graphics Output Protocol backed by a real, addressable frame buffer.
///
/// Every configured `(horizontal, vertical)` resolution becomes one GOP mode. `Blt()` operates on
/// an actual pixel buffer sized to the active mode, so tests can read back exactly what production
/// code drew with [`FakeGop::pixel`], rather than only observing call counts.
#[repr(C)]
pub(crate) struct FakeGop {
    protocol: graphics_output::Protocol,
    // Kept alive so `protocol.mode` (computed from this box's address before it moved here) stays valid.
    mode_storage: Box<graphics_output::Mode>,
    state: RefCell<FakeGopState>,
}

impl FakeGop {
    /// Builds a leaked, `'static` fake GOP with one mode per `resolutions` entry, mode 0 active,
    /// and every mode settable.
    pub(crate) fn new(resolutions: &[(u32, u32)]) -> &'static FakeGop {
        Self::with_unsettable_modes(resolutions, &[])
    }

    /// Like [`Self::new`], but `SetMode()` fails with `EFI_DEVICE_ERROR` for any mode index listed
    /// in `unsettable_modes`. Used to exercise the "mode reports a resolution but cannot actually be
    /// set" fallback in [`super::console::mode`].
    pub(crate) fn with_unsettable_modes(resolutions: &[(u32, u32)], unsettable_modes: &[u32]) -> &'static FakeGop {
        Self::build(resolutions, unsettable_modes, false)
    }

    /// Like [`Self::new`], but `QueryMode()` reports success while writing a null `Info` pointer,
    /// simulating a misbehaving GOP implementation.
    pub(crate) fn with_null_mode_info(resolutions: &[(u32, u32)]) -> &'static FakeGop {
        Self::build(resolutions, &[], true)
    }

    fn build(resolutions: &[(u32, u32)], unsettable_modes: &[u32], report_null_mode_info: bool) -> &'static FakeGop {
        let mode_infos: Vec<_> = resolutions
            .iter()
            .map(|&(horizontal_resolution, vertical_resolution)| graphics_output::ModeInformation {
                version: 0,
                horizontal_resolution,
                vertical_resolution,
                pixel_format: graphics_output::PIXEL_BLT_ONLY,
                pixel_information: graphics_output::PixelBitmask {
                    red_mask: 0,
                    green_mask: 0,
                    blue_mask: 0,
                    reserved_mask: 0,
                },
                pixels_per_scan_line: horizontal_resolution,
            })
            .collect();
        let unsettable = (0..mode_infos.len()).map(|index| unsettable_modes.contains(&(index as u32))).collect();
        let initial_pixels = resolutions.first().map_or(0, |&(horizontal, vertical)| (horizontal * vertical) as usize);

        let mode_storage = Box::new(graphics_output::Mode {
            max_mode: mode_infos.len() as u32,
            mode: 0,
            info: core::ptr::null_mut(),
            size_of_info: 0,
            frame_buffer_base: 0,
            frame_buffer_size: 0,
        });
        let mode_ptr: *mut graphics_output::Mode = core::ptr::from_ref(mode_storage.as_ref()).cast_mut();

        let protocol = graphics_output::Protocol {
            query_mode: fake_query_mode,
            set_mode: fake_set_mode,
            blt: fake_blt,
            mode: mode_ptr,
        };

        let black = graphics_output::BltPixel { blue: 0, green: 0, red: 0, reserved: 0 };
        let state = RefCell::new(FakeGopState {
            mode_infos,
            unsettable,
            framebuffer: alloc::vec![black; initial_pixels],
            set_mode_calls: 0,
            report_null_mode_info,
        });

        Box::leak(Box::new(FakeGop { protocol, mode_storage, state }))
    }

    /// Returns a pointer to the fake's protocol interface, as if just located/opened.
    pub(crate) fn handle(&self) -> NonNull<graphics_output::Protocol> {
        NonNull::from(&self.protocol)
    }

    /// Returns a ready-to-use [`GopHandle`] over this fake.
    pub(crate) fn gop_handle(&self) -> GopHandle {
        // SAFETY: `self` is leaked to `'static` in `new`/`with_unsettable_modes`, so the interface
        // stays live for as long as any `GopHandle` built from it is used, matching `GopHandle::new`.
        unsafe { GopHandle::new(self.handle(), permissive_compat_memory_manager()) }
    }

    /// Returns the number of times `SetMode()` has been called.
    pub(crate) fn set_mode_calls(&self) -> usize {
        self.state.borrow().set_mode_calls
    }

    /// Reads back the pixel at (`x`, `y`) in the currently active mode's frame buffer.
    ///
    /// # Panics
    ///
    /// Panics if `x`/`y` fall outside the active mode's resolution.
    pub(crate) fn pixel(&self, x: usize, y: usize) -> graphics_output::BltPixel {
        let state = self.state.borrow();
        let stride = current_mode_info(&self.protocol, &state).horizontal_resolution as usize;
        *state.framebuffer.get(y * stride + x).expect("pixel coordinates within the active mode")
    }
}

/// Returns the [`graphics_output::ModeInformation`] for the mode `protocol.mode` currently reports
/// as active.
fn current_mode_info<'a>(
    protocol: &graphics_output::Protocol,
    state: &'a FakeGopState,
) -> &'a graphics_output::ModeInformation {
    // SAFETY: `protocol.mode` always points at the `Mode` a `FakeGop` keeps alive in `mode_storage`.
    let mode = unsafe { (*protocol.mode).mode };
    state.mode_infos.get(mode as usize).expect("active mode index is always in range")
}

unsafe extern "efiapi" fn fake_query_mode(
    this: *mut graphics_output::Protocol,
    mode_number: u32,
    size_of_info: *mut usize,
    info: *mut *mut graphics_output::ModeInformation,
) -> efi::Status {
    // SAFETY: `this` always points to the `protocol` field of a `FakeGop`, this module's only caller,
    // which is `#[repr(C)]` with `protocol` as its first field.
    let fake = unsafe { &*this.cast::<FakeGop>() };
    let state = fake.state.borrow();
    let Some(mode_info) = state.mode_infos.get(mode_number as usize) else {
        return efi::Status::UNSUPPORTED;
    };
    // SAFETY: `size_of_info`/`info` are valid out-parameters, per `GopHandle::query_mode`'s contract.
    unsafe {
        size_of_info.write(core::mem::size_of::<graphics_output::ModeInformation>());
        info.write(if state.report_null_mode_info {
            core::ptr::null_mut()
        } else {
            core::ptr::from_ref(mode_info).cast_mut()
        });
    }
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn fake_set_mode(this: *mut graphics_output::Protocol, mode_number: u32) -> efi::Status {
    // SAFETY: as in `fake_query_mode`.
    let fake = unsafe { &*this.cast::<FakeGop>() };
    let mut state = fake.state.borrow_mut();
    state.set_mode_calls += 1;

    let Some(mode_info) = state.mode_infos.get(mode_number as usize).copied() else {
        return efi::Status::UNSUPPORTED;
    };
    if *state.unsettable.get(mode_number as usize).unwrap_or(&false) {
        return efi::Status::DEVICE_ERROR;
    }

    let pixel_count = (mode_info.horizontal_resolution * mode_info.vertical_resolution) as usize;
    let black = graphics_output::BltPixel { blue: 0, green: 0, red: 0, reserved: 0 };
    state.framebuffer = alloc::vec![black; pixel_count];
    drop(state);

    // SAFETY: `fake.protocol.mode` points at the `Mode` kept alive in `fake.mode_storage`.
    unsafe { (*fake.protocol.mode).mode = mode_number };
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn fake_blt(
    this: *mut graphics_output::Protocol,
    buffer: *mut graphics_output::BltPixel,
    operation: graphics_output::BltOperation,
    src_x: usize,
    src_y: usize,
    dest_x: usize,
    dest_y: usize,
    width: usize,
    height: usize,
    delta: usize,
) -> efi::Status {
    // SAFETY: as in `fake_query_mode`.
    let fake = unsafe { &*this.cast::<FakeGop>() };
    let mut state = fake.state.borrow_mut();
    let stride = current_mode_info(&fake.protocol, &state).horizontal_resolution as usize;
    let row_pixels = if delta == 0 { width } else { delta / core::mem::size_of::<graphics_output::BltPixel>() };

    match operation {
        graphics_output::BLT_VIDEO_FILL => {
            // SAFETY: for `EfiBltVideoFill`, `buffer` holds exactly one fill-color pixel.
            let color = unsafe { *buffer };
            for row in 0..height {
                for col in 0..width {
                    if let Some(pixel) = state.framebuffer.get_mut((dest_y + row) * stride + dest_x + col) {
                        *pixel = color;
                    }
                }
            }
        }
        graphics_output::BLT_VIDEO_TO_VIDEO => {
            let snapshot = state.framebuffer.clone();
            for row in 0..height {
                for col in 0..width {
                    let Some(&value) = snapshot.get((src_y + row) * stride + src_x + col) else { continue };
                    if let Some(pixel) = state.framebuffer.get_mut((dest_y + row) * stride + dest_x + col) {
                        *pixel = value;
                    }
                }
            }
        }
        graphics_output::BLT_VIDEO_TO_BLT_BUFFER => {
            for row in 0..height {
                for col in 0..width {
                    let Some(&value) = state.framebuffer.get((src_y + row) * stride + src_x + col) else { continue };
                    // SAFETY: `buffer` holds `height` rows of `row_pixels` pixels, per `Delta`'s contract.
                    unsafe { buffer.add(row * row_pixels + col).write(value) };
                }
            }
        }
        graphics_output::BLT_BUFFER_TO_VIDEO => {
            for row in 0..height {
                for col in 0..width {
                    // SAFETY: as above, for the read side.
                    let value = unsafe { buffer.add(row * row_pixels + col).read() };
                    if let Some(pixel) = state.framebuffer.get_mut((dest_y + row) * stride + dest_x + col) {
                        *pixel = value;
                    }
                }
            }
        }
        _ => return efi::Status::INVALID_PARAMETER,
    }
    efi::Status::SUCCESS
}

/// One recorded call to [`FakeHiiFont`]'s `StringToImage()`.
#[derive(Debug, Clone)]
pub(crate) struct DrawCall {
    /// The code units requested to be drawn, excluding the NUL terminator.
    pub(crate) text: Vec<u16>,
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) foreground: graphics_output::BltPixel,
    pub(crate) background: graphics_output::BltPixel,
}

/// Mutable state behind a [`FakeHiiFont`].
struct FakeHiiFontState {
    unknown_glyphs: Vec<u16>,
    force_error: Option<efi::Status>,
    draw_calls: Vec<DrawCall>,
}

/// A fake EFI HII Font Protocol.
///
/// Code units listed as "unknown" report no glyph (`GetGlyph()` fails, `StringToImage()` returns
/// `EFI_WARN_UNKNOWN_GLYPH`). Every other code unit is treated as renderable. `StringToImage()`
/// calls are recorded (see [`FakeHiiFont::draw_calls`]) instead of actually rasterizing, since the
/// pixel-level rendering is HII Font's own responsibility, not this crate's.
#[repr(C)]
pub(crate) struct FakeHiiFont {
    protocol: hii_font::Protocol,
    state: RefCell<FakeHiiFontState>,
}

impl FakeHiiFont {
    /// Builds a leaked, `'static` fake HII Font where every glyph is known.
    pub(crate) fn new() -> &'static FakeHiiFont {
        Self::build(&[], None)
    }

    /// Like [`Self::new`], but every code unit in `unknown_glyphs` is reported as having no glyph.
    pub(crate) fn with_unknown_glyphs(unknown_glyphs: &[u16]) -> &'static FakeHiiFont {
        Self::build(unknown_glyphs, None)
    }

    /// Builds a fake HII Font whose `StringToImage()` always fails with `status`.
    ///
    /// `status` must not be `EFI_SUCCESS` or `EFI_WARN_UNKNOWN_GLYPH`, which have their own,
    /// dedicated constructors.
    pub(crate) fn failing(status: efi::Status) -> &'static FakeHiiFont {
        Self::build(&[], Some(status))
    }

    fn build(unknown_glyphs: &[u16], force_error: Option<efi::Status>) -> &'static FakeHiiFont {
        let protocol = hii_font::Protocol {
            string_to_image: fake_string_to_image,
            string_id_to_image: fake_string_id_to_image,
            get_glyph: fake_get_glyph,
            get_font_info: fake_get_font_info,
        };
        let state = RefCell::new(FakeHiiFontState {
            unknown_glyphs: unknown_glyphs.to_vec(),
            force_error,
            draw_calls: Vec::new(),
        });
        Box::leak(Box::new(FakeHiiFont { protocol, state }))
    }

    /// Returns a pointer to the fake's protocol interface, as if just located.
    pub(crate) fn handle(&self) -> NonNull<hii_font::Protocol> {
        NonNull::from(&self.protocol)
    }

    /// Returns a ready-to-use [`HiiFontHandle`] over this fake.
    pub(crate) fn hii_font_handle(&self) -> HiiFontHandle {
        // SAFETY: `self` is leaked to `'static` in `build`, so the interface stays live for as long
        // as any `HiiFontHandle` built from it is used, matching `HiiFontHandle::new`.
        unsafe { HiiFontHandle::new(self.handle()) }
    }

    /// Returns every `StringToImage()` call recorded so far, oldest first.
    pub(crate) fn draw_calls(&self) -> Vec<DrawCall> {
        self.state.borrow().draw_calls.clone()
    }
}

unsafe extern "efiapi" fn fake_string_to_image(
    this: *const hii_font::Protocol,
    _flags: hii_font::OutFlags,
    string: hii_font::String,
    display_info: *const hii_font_ex::DisplayInfo,
    _image_output: *mut *mut hii_font_ex::ImageOutput,
    x: usize,
    y: usize,
    row_infos: *mut *mut hii_font::RowInfo,
    row_infos_count: *mut usize,
    _line_count: *mut usize,
) -> efi::Status {
    // SAFETY: `this` always points to the `protocol` field of a `FakeHiiFont`, this module's only
    // caller, which is `#[repr(C)]` with `protocol` as its first field.
    let fake = unsafe { &*this.cast::<FakeHiiFont>() };
    if !row_infos.is_null() {
        // SAFETY: a valid out-parameter per `StringToImage()`'s contract; no row info is produced.
        unsafe { row_infos.write(core::ptr::null_mut()) };
    }
    if !row_infos_count.is_null() {
        // SAFETY: as above.
        unsafe { row_infos_count.write(0) };
    }
    // SAFETY: `string` is a NUL-terminated CHAR16 string, per `HiiFontHandle::draw_to_screen`'s contract.
    let text = unsafe { patina::string::Char16Str::from_ptr(string) }.expect("valid test string");
    // SAFETY: `display_info` is a valid, fully initialized `DisplayInfo`, per the same contract.
    let display_info = unsafe { &*display_info };

    let mut state = fake.state.borrow_mut();
    let units = text.as_units();
    let has_unknown_glyph = units.iter().any(|unit| state.unknown_glyphs.contains(unit));
    state.draw_calls.push(DrawCall {
        text: units.to_vec(),
        x,
        y,
        foreground: display_info.foreground_color,
        background: display_info.background_color,
    });

    if let Some(status) = state.force_error {
        return status;
    }
    if has_unknown_glyph { efi::Status::WARN_UNKNOWN_GLYPH } else { efi::Status::SUCCESS }
}

unsafe extern "efiapi" fn fake_get_glyph(
    this: *const hii_font::Protocol,
    char_value: efi::Char16,
    _display_info: *const hii_font_ex::DisplayInfo,
    _image_output: *mut *mut hii_font_ex::ImageOutput,
    cell_info: *mut usize,
) -> efi::Status {
    // SAFETY: as in `fake_string_to_image`.
    let fake = unsafe { &*this.cast::<FakeHiiFont>() };
    if !cell_info.is_null() {
        // SAFETY: a valid out-parameter per `GetGlyph()`'s contract; unused by the caller under test.
        unsafe { cell_info.write(0) };
    }
    if fake.state.borrow().unknown_glyphs.contains(&char_value) { efi::Status::NOT_FOUND } else { efi::Status::SUCCESS }
}

/// Unused by any code this crate exercises; only present so [`hii_font::Protocol`] is fully populated.
unsafe extern "efiapi" fn fake_string_id_to_image(
    _this: *const hii_font::Protocol,
    _flags: hii_font::OutFlags,
    _package_list: efi::hii::Handle,
    _string_id: efi::hii::StringId,
    _language: *const efi::Char8,
    _display_info: *const hii_font_ex::DisplayInfo,
    _image_output: *mut *mut hii_font_ex::ImageOutput,
    _x: usize,
    _y: usize,
    _row_infos: *mut *mut hii_font::RowInfo,
    _row_infos_count: *mut usize,
    _line_count: *mut usize,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

/// Unused by any code this crate exercises; only present so [`hii_font::Protocol`] is fully populated.
unsafe extern "efiapi" fn fake_get_font_info(
    _this: *const hii_font::Protocol,
    _handle: *mut hii_font::Handle,
    _string_info_in: *const hii_font_ex::DisplayInfo,
    _string_info_out: *mut *mut hii_font_ex::DisplayInfo,
    _string: hii_font::String,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}
