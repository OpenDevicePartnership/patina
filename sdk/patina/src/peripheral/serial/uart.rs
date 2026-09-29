//! [`SerialIO`](crate::peripheral::serial::SerialIO) UART implementations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod null;
pub use null::UartNull;

#[cfg(target_arch = "x86_64")]
mod uart_16550;
#[cfg(target_arch = "x86_64")]
pub use uart_16550::Uart16550;

mod pl011;
pub use pl011::UartPl011;
