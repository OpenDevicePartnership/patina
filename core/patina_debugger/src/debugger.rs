//! Debugger struct implementation
//!
//! This modules contains the implementation of the Debugger trait. This implementation
//! will manage the high level orchestration of the debugger, including initializing
//! the debugger, handling exceptions, and managing the GDB state machine.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "alloc")]
use alloc::boxed::Box;
use gdbstub::{
    conn::{Connection, ConnectionExt},
    stub::{GdbStubBuilder, SingleThreadStopReason, state_machine::GdbStubStateMachine},
};
use patina::{component::service::perf_timer::ArchTimerFunctionality, serial::SerialIO};
use patina_internal_cpu::interrupts::{ExceptionType, HandlerType, InterruptHandler, InterruptManager};
use spin::Mutex;

use crate::{
    DebugError, Debugger, DebuggerLoggingPolicy, ExceptionInfo,
    arch::{DebuggerArch, SystemArch},
    dbg_target::PatinaTarget,
    system::SystemState,
    transport::{LoggingSuspender, SerialConnection},
};

/// Length of the static buffer used for GDB communication.
const GDB_BUFF_LEN: usize = 0x2000;

/// A default GDB stop packet used when entering the debugger.
const GDB_STOP_PACKET: &[u8] = b"$T05thread:01;#07";
const GDB_NACK_PACKET: &[u8] = b"-";

#[cfg(not(feature = "alloc"))]
static GDB_BUFFER: [u8; GDB_BUFF_LEN] = [0; GDB_BUFF_LEN];

// SAFETY: The exception info is not actually stored globally, but this is needed to satisfy
// the compiler as it will be a contained within the target struct which the GdbStub
// is generalized on using phantom data. This data will not actually be stored outside
// of the appropriate stack references.
unsafe impl Send for ExceptionInfo {}
// SAFETY: See above comment.
unsafe impl Sync for ExceptionInfo {}

/// Patina Debugger
///
/// This struct implements the Debugger trait for the Patina debugger. It wraps
/// a SerialIO transport and manages the debugger in an internal struct.
///
pub struct PatinaDebugger<T>
where
    T: SerialIO + 'static,
{
    /// The transport for the debugger.
    transport: T,
    /// The exception types the debugger will register for.
    exception_types: &'static [usize],
    /// Controls what the debugger does with logging.
    log_policy: DebuggerLoggingPolicy,
    /// Whether initializing the transport should be skipped.
    no_transport_init: bool,
    /// Debugger enabled state.
    enabled: AtomicBool,
    /// The number of seconds to wait for an initial breakpoint. If zero, wait indefinitely.
    initial_break_timeout: u32,
    /// Internal mutable debugger state.
    internal: Mutex<DebuggerInternal<'static, T>>,
    /// Tracks external system state.
    system_state: Mutex<SystemState>,
    /// Indicates that the previous connection timed out. Used to inform the next connection to print a hint.
    connection_timed_out: AtomicBool,
}

/// Internal Debugger State
///
/// contains the internal configuration and state for the debugger. This will
/// be locked to allow mutable access while using the debugger.
///
struct DebuggerInternal<'a, T>
where
    T: SerialIO,
{
    gdb: Option<GdbStubStateMachine<'a, PatinaTarget, SerialConnection<'a, T>>>,
    gdb_buffer: Option<&'a [u8; GDB_BUFF_LEN]>,
    timer: Option<&'a dyn ArchTimerFunctionality>,
    initial_breakpoint: bool,
}

impl<T: SerialIO> PatinaDebugger<T> {
    /// Create a new Patina debugger
    ///
    /// Creates a new Patina debugger instance with the provided transport.
    ///
    pub const fn new(transport: T) -> Self {
        PatinaDebugger {
            transport,
            log_policy: DebuggerLoggingPolicy::SuspendLogging,
            no_transport_init: false,
            exception_types: SystemArch::DEFAULT_EXCEPTION_TYPES,
            enabled: AtomicBool::new(false),
            initial_break_timeout: 0,
            internal: Mutex::new(DebuggerInternal {
                gdb_buffer: None,
                gdb: None,
                timer: None,
                initial_breakpoint: false,
            }),
            system_state: Mutex::new(SystemState::new()),
            connection_timed_out: AtomicBool::new(false),
        }
    }

    /// Forces the debugger to be enabled, regardless of later configuration. This
    /// is used for development purposes and is not intended for production or
    /// standard use. If `False` is provided, this routine will not change the configuration.
    pub const fn with_force_enable(mut self, enabled: bool) -> Self {
        if enabled {
            self.enabled = AtomicBool::new(true);
        }
        self
    }

    /// Configures the logging policy for the debugger. See [`DebuggerLoggingPolicy`]
    /// for more information on the available policies. By default, the debugger
    /// will suspend logging while broken in.
    pub const fn with_log_policy(mut self, policy: DebuggerLoggingPolicy) -> Self {
        self.log_policy = policy;
        self
    }

    /// Prevents the debugger from initializing the transport. This is suggested in
    /// cases where the transport is shared with the logging device.
    pub const fn without_transport_init(mut self) -> Self {
        self.no_transport_init = true;
        self
    }

    /// Customizes the exception types for which the debugger will be invoked.
    pub const fn with_exception_types(mut self, exception_types: &'static [usize]) -> Self {
        self.exception_types = exception_types;
        self
    }

    /// Configures the timeout for the initial breakpoint.
    ///
    /// `timeout_seconds` - Timeout specified in seconds. Zero indicates to wait indefinitely.
    pub const fn with_timeout(mut self, timeout_seconds: u32) -> Self {
        self.initial_break_timeout = timeout_seconds;
        self
    }

    /// Enables the debugger.
    ///
    /// Allows runtime enablement of the debugger. This should be called before the Patina
    /// core is invoked.
    ///
    /// Enabled - Whether the debugger is enabled, and will install itself into the system.
    ///
    pub fn enable(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Enters the debugger from an exception.
    fn enter_debugger(
        &'static self,
        exception_info: ExceptionInfo,
        restart: bool,
    ) -> Result<ExceptionInfo, DebugError> {
        let mut debug = match self.internal.try_lock() {
            Some(inner) => inner,
            None => return Err(DebugError::Reentry),
        };

        let mut target = PatinaTarget::new(exception_info, &self.system_state);
        let timeout = match debug.initial_breakpoint {
            true => {
                debug.initial_breakpoint = false;
                self.initial_break_timeout
            }
            false => 0,
        };

        // Either take the existing state machine, or start one if this is the first break.
        let mut gdb = match debug.gdb {
            Some(_) => debug.gdb.take().unwrap(),
            None => {
                let const_buffer = debug.gdb_buffer.ok_or(DebugError::NotInitialized)?;

                // Flush any stale data from the transport.
                while self.transport.try_read().is_some() {}

                // SAFETY: The buffer will only ever be used by the paired GDB stub
                // within the internal state lock. Because there is no GDB stub at
                // this point, there is no other references to the buffer. This
                // ensures a single locked mutable reference to the buffer.
                let mut_buffer =
                    unsafe { core::slice::from_raw_parts_mut(const_buffer.as_ptr() as *mut u8, const_buffer.len()) };

                let conn = SerialConnection::new(&self.transport);

                let builder = GdbStubBuilder::new(conn)
                    .with_packet_buffer(mut_buffer)
                    .build()
                    .map_err(|_| DebugError::GdbStubInit)?;

                builder.run_state_machine(&mut target).map_err(|_| DebugError::GdbStubInit)?
            }
        };

        let mut timeout_reached = false;
        if let GdbStubStateMachine::Idle(mut inner) = gdb {
            // If this is a restart, send a nack to request a resend of the failing packet.
            // Otherwise, always start with a stop code if starting from idle. This may be because this is the initial breakpoint
            // or because the initial breakpoint timed out. This is not to spec, but is a useful hint to the client
            // that a break has occurred. This allows the debugger to reconnect on scenarios like reboots.
            if restart {
                let _ = inner.borrow_conn().write_all(GDB_NACK_PACKET);
            } else {
                let _ = inner.borrow_conn().write_all(GDB_STOP_PACKET);
            }

            // Until some traffic is received, wait for the timeout before entering the state machine.
            if timeout != 0
                && let Some(timer) = debug.timer
            {
                let frequency = timer.perf_frequency();
                let initial_count = timer.cpu_count();
                loop {
                    if (timer.cpu_count() - initial_count) / frequency >= timeout as u64 {
                        timeout_reached = true;
                        break;
                    }

                    if !matches!(inner.borrow_conn().peek(), Ok(None)) {
                        // Data received, continue to the state machine.
                        break;
                    }
                }
            }

            gdb = GdbStubStateMachine::Idle(inner);
        }

        // Enter the state machine until the target is resumed or a timeout occurs.
        while !target.is_resumed() && !timeout_reached {
            gdb = match gdb {
                GdbStubStateMachine::Idle(mut gdb) => {
                    let byte = loop {
                        match gdb.borrow_conn().read() {
                            Ok(0x0) => {
                                log::warn!(
                                    "Debugger: Read 0x00 from the transport. This is unexpected and will be ignored."
                                );
                                continue;
                            }
                            Ok(b) => break b,
                            Err(_) => return Err(DebugError::TransportFailure),
                        }
                    };

                    match gdb.incoming_data(&mut target, byte) {
                        Ok(gdb) => gdb,
                        Err(e) => return Err(DebugError::GdbStubError(e)),
                    }
                }
                GdbStubStateMachine::Running(gdb) => {
                    // Windbg doesn't handle many stop reasons well, this could be improved in the future and
                    // wrapped in the windbg workarounds feature.
                    match gdb.report_stop(
                        &mut target,
                        SingleThreadStopReason::SignalWithThread { tid: (), signal: gdbstub::common::Signal::SIGTRAP },
                    ) {
                        Ok(gdb) => gdb,
                        Err(e) => return Err(DebugError::GdbStubError(e)),
                    }
                }
                GdbStubStateMachine::CtrlCInterrupt(gdb) => {
                    match gdb.interrupt_handled(&mut target, None::<SingleThreadStopReason<u64>>) {
                        Ok(gdb) => gdb,
                        Err(e) => return Err(DebugError::GdbStubError(e)),
                    }
                }
                GdbStubStateMachine::Disconnected(gdb) => gdb.return_to_idle(),
            };
        }

        if timeout_reached {
            self.connection_timed_out.store(true, Ordering::Relaxed);
        }

        if target.reboot_on_resume() {
            // Reboot the system.
            SystemArch::reboot();
            return Err(DebugError::RebootFailure);
        }

        // Target is resumed, store the state machine for the next break and
        // return the updated exception info.
        debug.gdb = Some(gdb);
        Ok(target.into_exception_info())
    }
}

impl<T: SerialIO> Debugger for PatinaDebugger<T> {
    fn initialize(
        &'static self,
        interrupt_manager: &mut dyn InterruptManager,
        timer: Option<&'static dyn ArchTimerFunctionality>,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::info!("Debugger is disabled.");
            return;
        }

        log::info!("Initializing debugger.");

        // Initialize the underlying transport.
        if !self.no_transport_init {
            self.transport.init();
        }

        // Initialize any architecture specifics.
        SystemArch::initialize();

        // Initialize the communication buffer.
        {
            let mut internal = self.internal.lock();
            cfg_if::cfg_if! {
                if #[cfg(feature = "alloc")] {
                    if internal.gdb_buffer.is_none() {
                        internal.gdb_buffer = Some(Box::leak(Box::new([0u8; GDB_BUFF_LEN])));
                    }
                }
                else {
                    internal.gdb_buffer = unsafe { Some(&*(GDB_BUFFER.as_ptr() as *mut [u8; GDB_BUFF_LEN])) };
                }
            }

            if timer.is_none() && self.initial_break_timeout != 0 {
                log::warn!(
                    "Debugger initialized with an initial break timeout but no timer service. Ignoring timeout."
                );
            }

            internal.timer = timer;
            internal.initial_breakpoint = true;
        }

        // Setup Exception Handlers.
        for exception_type in self.exception_types {
            // Remove the existing handler. Don't care about the return since
            // there may not be a handler anyways.
            let _ = interrupt_manager.unregister_exception_handler(*exception_type);

            let res = interrupt_manager.register_exception_handler(*exception_type, HandlerType::Handler(self));
            if res.is_err() {
                log::error!("Failed to register debugger exception handler for type {exception_type}: {res:?}");
            }
        }

        log::error!("************************************");
        log::error!("***  Initial debug breakpoint!   ***");
        log::error!("************************************");
        SystemArch::breakpoint();
        log::info!("Resuming from initial breakpoint.");
    }

    fn enabled(&'static self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn notify_module_load(&'static self, module_name: &str, address: usize, length: usize) {
        if !self.enabled() {
            return;
        }

        let breakpoint = {
            let mut state = self.system_state.lock();
            state.modules.add_module(module_name, address, length);
            state.modules.check_module_breakpoints(module_name)
        };

        if breakpoint {
            log::error!("MODULE BREAKPOINT! {module_name} - 0x{address:x} - 0x{length:x}");
            SystemArch::breakpoint();
        }
    }

    fn poll_debugger(&'static self) {
        const CRTL_C: u8 = 3;

        if !self.enabled() {
            return;
        }

        while let Some(byte) = self.transport.try_read() {
            if byte == CRTL_C {
                // Ctrl-C
                SystemArch::breakpoint();
            }
        }
    }

    fn add_monitor_command(
        &'static self,
        command: &'static str,
        description: &'static str,
        callback: crate::MonitorCommandFn,
    ) {
        if !self.enabled() {
            return;
        }

        self.system_state.lock().add_monitor_command(command, description, callback);
    }
}

impl<T: SerialIO> InterruptHandler for PatinaDebugger<T> {
    fn handle_interrupt(
        &'static self,
        exception_type: ExceptionType,
        context: &mut patina_internal_cpu::interrupts::ExceptionContext,
    ) {
        // Check if the previous connection timed out to print a hint.
        if self.connection_timed_out.swap(false, Ordering::Relaxed) {
            log::error!("********* DEBUGGER BREAK-IN *********");
        }

        // Suspend or disable logging. If suspended, logging will resume when the struct is dropped.
        let _log_suspend;
        match self.log_policy {
            DebuggerLoggingPolicy::SuspendLogging => {
                _log_suspend = LoggingSuspender::suspend();
            }
            DebuggerLoggingPolicy::DisableLogging => {
                log::set_max_level(log::LevelFilter::Off);
            }
            DebuggerLoggingPolicy::FullLogging => {
                // No action needed.
            }
        }

        // Check for a poke test before continuing
        if SystemArch::check_memory_poke_test(context) {
            log::info!("Memory poke test triggered, ignoring exception.");
            return;
        }

        let mut restart = false;
        let mut exception_info = loop {
            let exception_info = SystemArch::process_entry(exception_type as u64, context);
            let result = self.enter_debugger(exception_info, restart);

            match result {
                Ok(info) => break info,
                Err(DebugError::GdbStubError(gdb_error)) => {
                    // Restarting the debugger will reset any changes made to the
                    // context due to the way that information is owned in the stub,
                    // but this is better than crashing. If this proves problematic,
                    // a more robust solution could be explored. This will also
                    // resend the break packet to the client.
                    log::error!("GDB Stub error, restarting debugger. {gdb_error:?}");
                    restart = true;
                    continue;
                }
                Err(error) => {
                    // Other errors are not currently recoverable.
                    debugger_crash(error, exception_type);
                }
            }
        };

        SystemArch::process_exit(&mut exception_info);
        *context = exception_info.context;
    }
}

fn debugger_crash(error: DebugError, exception_type: ExceptionType) -> ! {
    // Always log crashes, the debugger will stop working anyways.
    log::set_max_level(log::LevelFilter::Error);
    log::error!("DEBUGGER CRASH! Error: {error:?} Exception Type: {exception_type:?}");

    // Could use SystemArch::reboot() in the future, but looping makes diagnosing
    // debugger bugs easier for now.
    #[allow(clippy::empty_loop)]
    loop {}
}
