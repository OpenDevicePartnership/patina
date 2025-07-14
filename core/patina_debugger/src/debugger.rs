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
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#[cfg(feature = "alloc")]
use alloc::boxed::Box;
use gdbstub::{
    conn::ConnectionExt,
    stub::{GdbStubBuilder, SingleThreadStopReason, state_machine::GdbStubStateMachine},
};
use patina_internal_cpu::interrupts::{ExceptionType, HandlerType, InterruptHandler, InterruptManager};
use patina_sdk::serial::SerialIO;
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

#[cfg(not(feature = "alloc"))]
static GDB_BUFFER: [u8; GDB_BUFF_LEN] = [0; GDB_BUFF_LEN];

// SAFETY: The exception info is not actually stored globally, but this is needed to satisfy
// the compiler as it will be a contained within the target struct which the GdbStub
// is generalized on using phantom data. This data will not actually be stored outside
// of the appropriate stack references.
unsafe impl Send for ExceptionInfo {}
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
    /// Internal mutable debugger config.
    config: spin::RwLock<DebuggerConfig>,
    /// Internal mutable debugger state.
    internal: Mutex<DebuggerInternal<'static, T>>,
    /// Tracks external system state.
    system_state: Mutex<SystemState>,
}

/// Debugger Configuration
///
/// contains the internal configuration and state for the debugger. This will
/// be locked to allow mutable access while using the debugger.
///
struct DebuggerConfig {
    enabled: bool,
    initial_break: bool,
    initial_break_timeout: u32,
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
            config: spin::RwLock::new(DebuggerConfig {
                enabled: false,
                initial_break: false,
                initial_break_timeout: 0,
            }),
            internal: Mutex::new(DebuggerInternal { gdb_buffer: None, gdb: None }),
            system_state: Mutex::new(SystemState::new()),
        }
    }

    /// Forces the debugger to be enabled, regardless of later configuration. This
    /// is used for development purposes and is not intended for production or
    /// standard use. If `False` is provided, this routine will not change the configuration.
    ///
    /// This will also forcibly enable the initial breakpoint with no timeout. This
    /// is intentional to prevent this development feature from being used in production.
    ///
    pub const fn with_force_enable(mut self, enabled: bool) -> Self {
        if enabled {
            // Intentionally ignoring initial_break config until configuration is thought out.
            self.config = spin::RwLock::new(DebuggerConfig { enabled, initial_break: true, initial_break_timeout: 0 });
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

    /// Configure the debugger.
    ///
    /// Allows runtime configuration of some of the debugger settings.
    ///
    /// Enabled - Whether the debugger is enabled, and will install itself into the system.
    ///
    /// Initial Break - Whether the debugger should break on initialization.
    ///
    /// Initial Break Timeout - A duration in seconds for the debugger to wait for a connection.
    /// 0 indicates no timeout and will wait indefinitely
    ///
    pub fn configure(&self, enabled: bool, _initial_break: bool, _initial_break_timeout: u32) {
        let mut config = self.config.write();
        config.enabled = enabled;
        // Intentionally ignoring initial_break config until configuration is thought out.
        config.initial_break = true;
    }

    /// Enters the debugger from an exception.
    fn enter_debugger(&'static self, exception_info: ExceptionInfo) -> Result<ExceptionInfo, DebugError> {
        let mut debug = match self.internal.try_lock() {
            Some(inner) => inner,
            None => return Err(DebugError::Reentry),
        };

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

        let mut target = PatinaTarget::new(exception_info, &self.system_state);

        // Either take the existing state machine, or start one if this is the first break.
        let mut gdb = match debug.gdb {
            Some(_) => debug.gdb.take().unwrap(),
            None => {
                let const_buffer = debug.gdb_buffer.ok_or(DebugError::NotInitialized)?;

                // Flush any stale data from the transport.
                while self.transport.try_read().is_some() {}

                // Always start with a stop code. This is not to spec, but is a
                // useful hint to the client that a break has occurred. This allows
                // the debugger to reconnect on scenarios like reboots.
                self.transport.write("$T05thread:01;#07".as_bytes());

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

        // Enter the state machine until the target is resumed.
        while !target.is_resumed() {
            gdb = match gdb {
                GdbStubStateMachine::Idle(mut gdb) => {
                    let byte = loop {
                        match gdb.borrow_conn().read() {
                            Ok(0x0) => {
                                log::error!(
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
    fn initialize(&'static self, interrupt_manager: &mut dyn InterruptManager) {
        let config = self.config.read();
        if !config.enabled {
            log::info!("Debugger is disabled.");
            return;
        }

        log::info!("Initializing debugger.");
        let initial_breakpoint = config.initial_break;
        let _initial_break_timeout = config.initial_break_timeout; // TODO

        // Drop the lock to prevent deadlock in the initial breakpoint.
        drop(config);

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
                    internal.monitor_buffer = unsafe { Some(&*(MONITOR_BUFFER.as_ptr() as *mut [u8; MONITOR_BUFF_LEN])) };
                }
            }
        }

        // Setup Exception Handlers.
        for exception_type in self.exception_types {
            // Remove the existing handler. Don't care about the return since
            // there may not be a handler anyways.
            let _ = interrupt_manager.unregister_exception_handler(*exception_type);

            let res = interrupt_manager.register_exception_handler(*exception_type, HandlerType::Handler(self));
            if res.is_err() {
                log::error!("Failed to register debugger exception handler for type {}: {:?}", exception_type, res);
            }
        }

        if initial_breakpoint {
            log::error!("************************************");
            log::error!("***  Initial debug breakpoint!   ***");
            log::error!("************************************");
            SystemArch::breakpoint();
            log::info!("Resuming from initial breakpoint.");
        }
    }

    fn enabled(&'static self) -> bool {
        self.config.read().enabled
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
            log::error!("MODULE BREAKPOINT! {} - 0x{:x} - 0x{:x}", module_name, address, length);
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

    fn add_monitor_command(&'static self, command: &'static str, callback: crate::MonitorCommandFn) {
        if !self.enabled() {
            return;
        }

        self.system_state.lock().add_monitor_command(command, callback);
    }
}

impl<T: SerialIO> InterruptHandler for PatinaDebugger<T> {
    fn handle_interrupt(
        &'static self,
        exception_type: ExceptionType,
        context: &mut patina_internal_cpu::interrupts::ExceptionContext,
    ) {
        let mut exception_info = SystemArch::process_entry(exception_type as u64, context);
        let result = self.enter_debugger(exception_info);

        exception_info = result.unwrap_or_else(|error| {
            // In the future, this could be make more robust by trying
            // to re-enter the debugger, re-initializing the stub. This
            // may require a new communication buffer though.
            debugger_crash(error, exception_type);
        });

        SystemArch::process_exit(&mut exception_info);
        *context = exception_info.context;
    }
}

fn debugger_crash(error: DebugError, exception_type: ExceptionType) -> ! {
    // Always log crashes, the debugger will stop working anyways.
    log::set_max_level(log::LevelFilter::Error);
    log::error!("DEBUGGER CRASH! Error: {:?} Exception Type: {:?}", error, exception_type);

    // Could use SystemArch::reboot() in the future, but looping makes diagnosing
    // debugger bugs easier for now.
    #[allow(clippy::empty_loop)]
    loop {}
}
