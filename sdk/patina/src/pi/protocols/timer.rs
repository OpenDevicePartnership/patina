//! Timer Architectural Protocol
//!
//! Used to set up a periodic timer interrupt using a platform specific timer, and a processor-specific interrupt
//! vector. This protocol enables the use of the SetTimer() Boot Service.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#timer-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x26BACCB3, 0x6F42, 0x11D4, 0xBC, 0xE7, &[0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81]);

/// A function of this type is called when a timer interrupt fires.  This
/// function executes at TPL_HIGH_LEVEL.  The DXE Core will register a function
/// of this type to be called for the timer interrupt, so it can know how much
/// time has passed.  This information is used to signal timer based events.
///
/// * time -  Time since the last timer interrupt in 100 ns units. This will
///   typically be TimerPeriod, but if a timer interrupt is missed, and the
///   EFI_TIMER_ARCH_PROTOCOL driver can detect missed interrupts, then Time
///   will contain the actual amount of time since the last interrupt.
pub type EfiTimerNotify = extern "efiapi" fn(time: u64);

/// This function registers the handler NotifyFunction so it is called every time
/// the timer interrupt fires.  It also passes the amount of time since the last
/// handler call to the NotifyFunction.  If NotifyFunction is NULL, then the
/// handler is unregistered.  If the handler is registered, then EFI_SUCCESS is
/// returned.  If the CPU does not support registering a timer interrupt handler,
/// then EFI_UNSUPPORTED is returned.  If an attempt is made to register a handler
/// when a handler is already registered, then EFI_ALREADY_STARTED is returned.
/// If an attempt is made to unregister a handler when a handler is not registered,
/// then EFI_INVALID_PARAMETER is returned.  If an error occurs attempting to
/// register the NotifyFunction with the timer interrupt, then EFI_DEVICE_ERROR
/// is returned.
///
/// * this - The EFI_TIMER_ARCH_PROTOCOL instance.
/// * notify_function - The function to call when a timer interrupt fires. This
///   function executes at TPL_HIGH_LEVEL. The DXE Core will register a handler for
///   the timer interrupt, so it can know how much time has passed. This information
///   is used to signal timer based events. NULL will unregister the handler.
/// * @retval - EFI_SUCCESS: The timer handler was registered.
/// * @retval - EFI_UNSUPPORTED: The platform does not support timer interrupts.
/// * @retval - EFI_ALREADY_STARTED: NotifyFunction is not NULL, and a handler is already
///   registered.
/// * @retval - EFI_INVALID_PARAMETER: NotifyFunction is NULL, and a handler was not
///   previously registered.
/// * @retval - EFI_DEVICE_ERROR: The timer handler could not be registered.
pub type EfiTimerRegisterHandler =
    extern "efiapi" fn(this: *mut Protocol, notify_function: EfiTimerNotify) -> efi::Status;

/// This function adjusts the period of timer interrupts to the value specified
/// by TimerPeriod.  If the timer period is updated, then the selected timer
/// period is stored in EFI_TIMER.TimerPeriod, and EFI_SUCCESS is returned.  If
/// the timer hardware is not programmable, then EFI_UNSUPPORTED is returned.
/// If an error occurs while attempting to update the timer period, then the
/// timer hardware will be put back in its state prior to this call, and
/// EFI_DEVICE_ERROR is returned.  If TimerPeriod is 0, then the timer interrupt
/// is disabled.  This is not the same as disabling the CPU's interrupts.
/// Instead, it must either turn off the timer hardware, or it must adjust the
/// interrupt controller so that a CPU interrupt is not generated when the timer
/// interrupt fires.
/// * this - The EFI_TIMER_ARCH_PROTOCOL instance.
/// * timer_period - The rate to program the timer interrupt in 100 nS units. If
///   the timer hardware is not programmable, then EFI_UNSUPPORTED is returned.
///   If the timer is programmable, then the timer period will be rounded up to
///   the nearest timer period that is supported by the timer hardware. If TimerPeriod
///   is set to 0, then the timer interrupts will be disabled.
/// * @retval - EFI_SUCCESS: The timer period was changed.
/// * @retval - EFI_UNSUPPORTED: The platform cannot change the period of the timer interrupt.
/// * @retval - EFI_DEVICE_ERROR: The timer period could not be changed due to a device error.
pub type EfiTimerSetTimerPeriod = extern "efiapi" fn(this: *mut Protocol, timer_period: u64) -> efi::Status;

/// This function retrieves the period of timer interrupts in 100 ns units,
/// returns that value in TimerPeriod, and returns EFI_SUCCESS.  If TimerPeriod
/// is NULL, then EFI_INVALID_PARAMETER is returned.  If a TimerPeriod of 0 is
/// returned, then the timer is currently disabled.
/// * this - The EFI_TIMER_ARCH_PROTOCOL instance.
/// * timer_period - A pointer to the timer period to retrieve in 100 ns units. If
///   0 is returned, then the timer is currently disabled.
/// * @retval - EFI_SUCCESS: The timer period was returned in TimerPeriod.
/// * @retval - EFI_INVALID_PARAMETER: TimerPeriod is NULL.
pub type EfiTimerGetTimerPeriod = extern "efiapi" fn(*mut Protocol, *mut u64) -> efi::Status;

/// This function generates a soft timer interrupt. If the platform does not support soft
/// timer interrupts, then EFI_UNSUPPORTED is returned. Otherwise, EFI_SUCCESS is returned.
/// If a handler has been registered through the EFI_TIMER_ARCH_PROTOCOL.RegisterHandler()
/// service, then a soft timer interrupt will be generated. If the timer interrupt is
/// enabled when this service is called, then the registered handler will be invoked. The
/// registered handler should not be able to distinguish a hardware-generated timer
/// interrupt from a software-generated timer interrupt.
/// * this - The EFI_TIMER_ARCH_PROTOCOL instance.
/// * @retval - EFI_SUCCESS: The soft timer interrupt was generated.
/// * @retval - EFI_UNSUPPORTED: The platform does not support the generation of soft timer interrupts.
pub type EfiTimerGenerateSoftInterrupt = extern "efiapi" fn(this: *mut Protocol) -> efi::Status;

/// This protocol provides the services to initialize a periodic timer interrupt, and to register a handler
/// that is called each time the time interrupt fires.  It may also provide a service to adjust the rate of the
/// periodic timer interrupt.  When a timer interrupt occurs, the handler is passed the amount of time that has
/// passed since the previous timer interrupt.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.10.1
#[repr(C)]
pub struct Protocol {
    pub register_handler: EfiTimerRegisterHandler,
    pub set_timer_period: EfiTimerSetTimerPeriod,
    pub get_timer_period: EfiTimerGetTimerPeriod,
    pub generate_soft_interrupt: EfiTimerGenerateSoftInterrupt,
}
