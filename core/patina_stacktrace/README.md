# Stack Trace Library

## Introduction

This library implements stack-walking logic. Given an instruction pointer and a
stack pointer, the [API](#public-api) dumps the stack trace that led to that
machine state. It currently does not resolve symbols because PDB debug
information is not embedded in the PE image, unlike DWARF data in ELF images.
Therefore, symbol resolution must be performed offline. As a result, the "Call
Site" column in the output displays `module+<relative pc>` instead of
`module!function+<relative pc>`. Outside of this library, with PDB access,
those module-relative PC offsets can be resolved to function-relative offsets,
as shown below.

```cmd
C:\> .\resolve_stacktrace.cmd
Enter the PDB directory path (leave empty to use STACKTRACE_PDB_DIR env): C:\temp\stacktrace
Enter stack trace lines (press Enter twice to finish):
WARN -       # Child-SP              Return Address         Call Site
WARN -       0 000001007E2796C0      000001007E27BBDC       qemu_sbsa_dxe_core+185DC
WARN -       1 000001007E2796F0      000001007E34FB58       qemu_sbsa_dxe_core+1BDC
WARN -       2 000001007E2797C0      000001007E34F6D0       qemu_sbsa_dxe_core+D5B54
WARN -       3 000001007E2797F0      000001007E35076C       qemu_sbsa_dxe_core+D56CC
WARN -       4 000001007E2798A0      000001007E29CEFC       qemu_sbsa_dxe_core+D676C
WARN -       5 000001007E2798B0      000001007E2A9CB0       qemu_sbsa_dxe_core+22EFC
WARN -       6 000001007E279920      000001007E2A5EF8       qemu_sbsa_dxe_core+2FCB0
WARN -       7 000001007E279A00      000001007E2A6628       qemu_sbsa_dxe_core+2BEF8
WARN -       8 000001007E279A90      000001007E2895A4       qemu_sbsa_dxe_core+2C628
WARN -       9 000001007E279AD0      000001007E28A1D8       qemu_sbsa_dxe_core+F5A4
WARN -      10 000001007E279BC0      000001007E27BE24       qemu_sbsa_dxe_core+101D8

# cspell:disable
┌────┬──────────────────────────────────────────────────────────────────────┬──────────────────┬──────────────────┬─────────────────────────────────────────────────────────────────────┐
│ #  ┆ Source Path                                                          ┆ Child-SP         ┆ Return Address   ┆ Call Site                                                           │
╞════╪══════════════════════════════════════════════════════════════════════╪══════════════════╪══════════════════╪═════════════════════════════════════════════════════════════════════╡
│ 0  ┆ /home/vineel/repos/patina/core/patina_stacktrace/src/stacktrace.rs @ ┆ 000001007E2796C0 ┆ 000001007E27BBDC ┆ qemu_sbsa_dxe_core!patina_stacktrace::stacktrace::StackTrace::dump( │
│    ┆ 144                                                                  ┆                  ┆                  ┆ )+0xC                                                               │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 1  ┆ /home/vineel/repos/patina-dxe-core-qemu/bin/sbsa_dxe_core.rs @ 24    ┆ 000001007E2796F0 ┆ 000001007E34FB58 ┆ qemu_sbsa_dxe_core!qemu_sbsa_dxe_core::panic(core::panic::panic_inf │
│    ┆                                                                      ┆                  ┆                  ┆ o::PanicInfo*)+0x88                                                 │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 2  ┆ /home/vineel/.rustup/toolchains/1.89.0-x86_64-unknown-linux-gnu/lib/ ┆ 000001007E2797C0 ┆ 000001007E34F6D0 ┆ qemu_sbsa_dxe_core!core::panicking::panic_fmt(core::fmt::Arguments, │
│    ┆ rustlib/src/rust/library/core/src/panicking.rs @ 75                  ┆                  ┆                  ┆ core::panic::location::Location*)+0x18                              │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 3  ┆ /home/vineel/repos/patina/components/patina_samples/src/component/he ┆ 000001007E2797F0 ┆ 000001007E35076C ┆ qemu_sbsa_dxe_core!patina_samples::component::hello_world::HelloStr │
│    ┆ llo_world.rs @ 23                                                    ┆                  ┆                  ┆ uct::entry_point(patina_samples::component::hello_world::HelloStruc │
│    ┆                                                                      ┆                  ┆                  ┆ t, patina::component::params::Config<i32>)+0xC4                     │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 4  ┆ /home/vineel/repos/patina/sdk/patina/src/component/params.rs @ 254   ┆ 000001007E2798A0 ┆ 000001007E29CEFC ┆ qemu_sbsa_dxe_core!patina::component::params::impl$20::run<patina_s │
│    ┆                                                                      ┆                  ┆                  ┆ amples::component::hello_world::HelloStruct,enum2$<core::result::Re │
│    ┆                                                                      ┆                  ┆                  ┆ sult<tuple$<>,enum2$<patina::error::EfiError> > >,enum2$<core::resu │
│    ┆                                                                      ┆                  ┆                  ┆ lt::Result<tuple$<>,enum2$<patina::error::EfiError> > > (*)(patina_ │
│    ┆                                                                      ┆                  ┆                  ┆ samples::component::hello_world::HelloStruct,patina::component::par │
│    ┆                                                                      ┆                  ┆                  ┆ ams::Config<i32>),patina::component::params::Config<i32> >(enum2$<c │
│    ┆                                                                      ┆                  ┆                  ┆ ore::result::Result<tuple$<>,enum2$<patina::error::EfiError> > >    │
│    ┆                                                                      ┆                  ┆                  ┆ (**)(patina_samples::component::hello_world::HelloStruct,           │
│    ┆                                                                      ┆                  ┆                  ┆ patina::component::params::Config<i32>), enum2$<core::option::Optio │
│    ┆                                                                      ┆                  ┆                  ┆ n<patina_samples::component::hello_world::HelloStruct> >*,          │
│    ┆                                                                      ┆                  ┆                  ┆ tuple$<patina::component::params::Config<i32> >)+0x18               │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 5  ┆ /home/vineel/repos/patina/sdk/patina/src/component/struct_component. ┆ 000001007E2798B0 ┆ 000001007E2A9CB0 ┆ qemu_sbsa_dxe_core!patina::component::struct_component::impl$1::run │
│    ┆ rs @ 94                                                              ┆                  ┆                  ┆ _unsafe<tuple$<patina::component::params::RunOnce,enum2$<core::resu │
│    ┆                                                                      ┆                  ┆                  ┆ lt::Result<tuple$<>,enum2$<patina::error::EfiError> > > (*)(patina_ │
│    ┆                                                                      ┆                  ┆                  ┆ samples::component::hello_world::HelloStruct,patina::component::par │
│    ┆                                                                      ┆                  ┆                  ┆ ams::Config<i32>)>,patina_samples::component::hello_world::HelloStr │
│    ┆                                                                      ┆                  ┆                  ┆ uct,enum2$<core::result::Result<tuple$<>,enum2$<patina::error::EfiE │
│    ┆                                                                      ┆                  ┆                  ┆ rror> > > (*)(patina_samples::component::hello_world::HelloStruct,p │
│    ┆                                                                      ┆                  ┆                  ┆ atina::component::params::Config<i32>)>(patina::component::struct_c │
│    ┆                                                                      ┆                  ┆                  ┆ omponent::StructComponent<tuple$<patina::component::params::RunOnce │
│    ┆                                                                      ┆                  ┆                  ┆ ,enum2$<core::result::Result<tuple$<>,enum2$<patina::error::EfiErro │
│    ┆                                                                      ┆                  ┆                  ┆ r> > > (*)(patina_samples::component::hello_world::HelloStruct,pati │
│    ┆                                                                      ┆                  ┆                  ┆ na::component::params::Config<i32>)>,enum2$<core::result::Result<tu │
│    ┆                                                                      ┆                  ┆                  ┆ ple$<>,enum2$<patina::error::EfiError> > > (*)(patina_samples::comp │
│    ┆                                                                      ┆                  ┆                  ┆ onent::hello_world::HelloStruct,patina::component::params::Config<i │
│    ┆                                                                      ┆                  ┆                  ┆ 32>)>*, patina::component::storage::UnsafeStorageCell)+0x58         │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 6  ┆ /home/vineel/repos/patina/patina_dxe_core/src/lib.rs @ 371           ┆ 000001007E279920 ┆ 000001007E2A5EF8 ┆ qemu_sbsa_dxe_core!patina_dxe_core::impl$4::dispatch_components::cl │
│    ┆                                                                      ┆                  ┆                  ┆ osure$0(patina_dxe_core::impl$4::dispatch_components::closure_env$0 │
│    ┆                                                                      ┆                  ┆                  ┆ *, alloc::boxed::Box<dyn$<patina::component::Component>,alloc::allo │
│    ┆                                                                      ┆                  ┆                  ┆ c::Global>*)+0xB8                                                   │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 7  ┆ /home/vineel/.rustup/toolchains/1.89.0-x86_64-unknown-linux-gnu/lib/ ┆ 000001007E279A00 ┆ 000001007E2A6628 ┆ qemu_sbsa_dxe_core!alloc::vec::impl$1::retain_mut::process_loop<pat │
│    ┆ rustlib/src/rust/library/alloc/src/vec/mod.rs @ 2243                 ┆                  ┆                  ┆ ina_dxe_core::impl$4::dispatch_components::closure_env$0,alloc::box │
│    ┆                                                                      ┆                  ┆                  ┆ ed::Box<dyn$<patina::component::Component>,alloc::alloc::Global>,al │
│    ┆                                                                      ┆                  ┆                  ┆ loc::alloc::Global,true>(unsigned long long,                        │
│    ┆                                                                      ┆                  ┆                  ┆ patina_dxe_core::impl$4::dispatch_components::closure_env$0*, alloc │
│    ┆                                                                      ┆                  ┆                  ┆ ::vec::impl$1::retain_mut::BackshiftOnDrop<alloc::boxed::Box<dyn$<p │
│    ┆                                                                      ┆                  ┆                  ┆ atina::component::Component>,alloc::alloc::Global>,alloc::alloc::Gl │
│    ┆                                                                      ┆                  ┆                  ┆ obal>*)+0x68                                                        │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 8  ┆ /home/vineel/.rustup/toolchains/1.89.0-x86_64-unknown-linux-gnu/lib/ ┆ 000001007E279A90 ┆ 000001007E2895A4 ┆ qemu_sbsa_dxe_core!alloc::vec::Vec<alloc::boxed::Box<dyn$<patina::c │
│    ┆ rustlib/src/rust/library/alloc/src/vec/mod.rs @ 2275                 ┆                  ┆                  ┆ omponent::Component>,alloc::alloc::Global>,alloc::alloc::Global>::r │
│    ┆                                                                      ┆                  ┆                  ┆ etain_mut<alloc::boxed::Box<dyn$<patina::component::Component>,allo │
│    ┆                                                                      ┆                  ┆                  ┆ c::alloc::Global>,alloc::alloc::Global,patina_dxe_core::impl$4::dis │
│    ┆                                                                      ┆                  ┆                  ┆ patch_components::closure_env$0>(patina_dxe_core::impl$4::dispatch_ │
│    ┆                                                                      ┆                  ┆                  ┆ components::closure_env$0)+0x44                                     │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 9  ┆ /home/vineel/repos/patina/patina_dxe_core/src/lib.rs @ 398           ┆ 000001007E279AD0 ┆ 000001007E28A1D8 ┆ qemu_sbsa_dxe_core!patina_dxe_core::Core<patina_dxe_core::Alloc>::c │
│    ┆                                                                      ┆                  ┆                  ┆ ore_dispatcher()+0xA4                                               │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 10 ┆ /home/vineel/repos/patina/patina_dxe_core/src/lib.rs @ 555           ┆ 000001007E279BC0 ┆ 000001007E27BE24 ┆ qemu_sbsa_dxe_core!patina_dxe_core::Core<patina_dxe_core::Alloc>::s │
│    ┆                                                                      ┆                  ┆                  ┆ tart(patina_dxe_core::Core<patina_dxe_core::Alloc>)+0xAF4           │
└────┴──────────────────────────────────────────────────────────────────────┴──────────────────┴──────────────────┴─────────────────────────────────────────────────────────────────────┘
# cspell:enable

C:\> .\resolve_stacktrace.cmd
Enter the PDB directory path (leave empty to use STACKTRACE_PDB_DIR env): C:\temp\stacktrace
Enter stack trace lines (press Enter twice to finish):
WARN -       # Child-SP              Return Address         Call Site
WARN -       0 000000007E8D4480      000000007E8DC5D5       qemu_q35_dxe_core+206B1
WARN -       1 000000007E8D44D0      000000007E9E6C32       qemu_q35_dxe_core+75D5
WARN -       2 000000007E8D45D0      000000007E9E65B2       qemu_q35_dxe_core+111C32
WARN -       3 000000007E8D4610      000000007E9E7DD1       qemu_q35_dxe_core+1115B2
WARN -       4 000000007E8D46E0      000000007E906FC8       qemu_q35_dxe_core+112DD1
WARN -       5 000000007E8D4710      000000007E9070F8       qemu_q35_dxe_core+31FC8
WARN -       6 000000007E8D47B0      000000007E915B3E       qemu_q35_dxe_core+320F8
WARN -       7 000000007E8D47F0      000000007E9114C8       qemu_q35_dxe_core+40B3E
WARN -       8 000000007E8D48F0      000000007E911D81       qemu_q35_dxe_core+3C4C8
WARN -       9 000000007E8D49A0      000000007E8EB7AE       qemu_q35_dxe_core+3CD81
WARN -      10 000000007E8D4A10      000000007E8EC7F6       qemu_q35_dxe_core+167AE
WARN -      11 000000007E8D4B10      000000007E8DCA9D       qemu_q35_dxe_core+177F6
WARN -      12 000000007E8D4D10      000000007EBE338F       qemu_q35_dxe_core+7A9D

# cspell:disable
┌────┬──────────────────────────────────────────────────────────────────────┬──────────────────┬──────────────────┬─────────────────────────────────────────────────────────────────────┐
│ #  ┆ Source Path                                                          ┆ Child-SP         ┆ Return Address   ┆ Call Site                                                           │
╞════╪══════════════════════════════════════════════════════════════════════╪══════════════════╪══════════════════╪═════════════════════════════════════════════════════════════════════╡
│ 0  ┆ E:\repos\patina\core\patina_stacktrace\src\stacktrace.rs @ 162       ┆ 000000007E8D4480 ┆ 000000007E8DC5D5 ┆ qemu_q35_dxe_core!patina_stacktrace::stacktrace::StackTrace::dump() │
│    ┆                                                                      ┆                  ┆                  ┆ +0xF                                                                │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 1  ┆ E:\repos\patina-dxe-core-qemu\bin\q35_dxe_core.rs @ 27               ┆ 000000007E8D44D0 ┆ 000000007E9E6C32 ┆ qemu_q35_dxe_core!qemu_q35_dxe_core::panic(core::panic::panic_info: │
│    ┆                                                                      ┆                  ┆                  ┆ :PanicInfo*)+0xB2                                                   │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 2  ┆ C:\Users\vineelko\.rustup\toolchains\1.89.0-x86_64-pc-windows-msvc\l ┆ 000000007E8D45D0 ┆ 000000007E9E65B2 ┆ qemu_q35_dxe_core!core::panicking::panic_fmt(core::fmt::Arguments,  │
│    ┆ ib\rustlib\src\rust\library\core\src\panicking.rs @ 75               ┆                  ┆                  ┆ core::panic::location::Location*)+0x1E                              │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 3  ┆ E:\repos\patina\components\patina_samples\src\component\hello_world. ┆ 000000007E8D4610 ┆ 000000007E9E7DD1 ┆ qemu_q35_dxe_core!patina_samples::component::hello_world::HelloStru │
│    ┆ rs @ 23                                                              ┆                  ┆                  ┆ ct::entry_point(patina_samples::component::hello_world::HelloStruct │
│    ┆                                                                      ┆                  ┆                  ┆ , patina::component::params::Config<i32>)+0x109                     │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 4  ┆ E:\repos\patina\sdk\patina\src\component\params.rs @ 254             ┆ 000000007E8D46E0 ┆ 000000007E906FC8 ┆ qemu_q35_dxe_core!patina::component::params::impl$20::run<patina_sa │
│    ┆                                                                      ┆                  ┆                  ┆ mples::component::hello_world::HelloStruct,enum2$<core::result::Res │
│    ┆                                                                      ┆                  ┆                  ┆ ult<tuple$<>,enum2$<patina::error::EfiError> > >,enum2$<core::resul │
│    ┆                                                                      ┆                  ┆                  ┆ t::Result<tuple$<>,enum2$<patina::error::EfiError> > > (*)(patina_s │
│    ┆                                                                      ┆                  ┆                  ┆ amples::component::hello_world::HelloStruct,patina::component::para │
│    ┆                                                                      ┆                  ┆                  ┆ ms::Config<i32>),patina::component::params::Config<i32> >(enum2$<co │
│    ┆                                                                      ┆                  ┆                  ┆ re::result::Result<tuple$<>,enum2$<patina::error::EfiError> > >     │
│    ┆                                                                      ┆                  ┆                  ┆ (**)(patina_samples::component::hello_world::HelloStruct,           │
│    ┆                                                                      ┆                  ┆                  ┆ patina::component::params::Config<i32>), enum2$<core::option::Optio │
│    ┆                                                                      ┆                  ┆                  ┆ n<patina_samples::component::hello_world::HelloStruct> >*,          │
│    ┆                                                                      ┆                  ┆                  ┆ tuple$<patina::component::params::Config<i32> >)+0x29               │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 5  ┆ E:\repos\patina\sdk\patina\src\component\struct_component.rs @ 94    ┆ 000000007E8D4710 ┆ 000000007E9070F8 ┆ qemu_q35_dxe_core!patina::component::struct_component::impl$1::run_ │
│    ┆                                                                      ┆                  ┆                  ┆ unsafe<tuple$<patina::component::params::RunOnce,enum2$<core::resul │
│    ┆                                                                      ┆                  ┆                  ┆ t::Result<tuple$<>,enum2$<patina::error::EfiError> > > (*)(patina_s │
│    ┆                                                                      ┆                  ┆                  ┆ amples::component::hello_world::HelloStruct,patina::component::para │
│    ┆                                                                      ┆                  ┆                  ┆ ms::Config<i32>)>,patina_samples::component::hello_world::HelloStru │
│    ┆                                                                      ┆                  ┆                  ┆ ct,enum2$<core::result::Result<tuple$<>,enum2$<patina::error::EfiEr │
│    ┆                                                                      ┆                  ┆                  ┆ ror> > > (*)(patina_samples::component::hello_world::HelloStruct,pa │
│    ┆                                                                      ┆                  ┆                  ┆ tina::component::params::Config<i32>)>(patina::component::struct_co │
│    ┆                                                                      ┆                  ┆                  ┆ mponent::StructComponent<tuple$<patina::component::params::RunOnce, │
│    ┆                                                                      ┆                  ┆                  ┆ enum2$<core::result::Result<tuple$<>,enum2$<patina::error::EfiError │
│    ┆                                                                      ┆                  ┆                  ┆ > > > (*)(patina_samples::component::hello_world::HelloStruct,patin │
│    ┆                                                                      ┆                  ┆                  ┆ a::component::params::Config<i32>)>,enum2$<core::result::Result<tup │
│    ┆                                                                      ┆                  ┆                  ┆ le$<>,enum2$<patina::error::EfiError> > > (*)(patina_samples::compo │
│    ┆                                                                      ┆                  ┆                  ┆ nent::hello_world::HelloStruct,patina::component::params::Config<i3 │
│    ┆                                                                      ┆                  ┆                  ┆ 2>)>*, patina::component::storage::UnsafeStorageCell)+0x63          │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 6  ┆ E:\repos\patina\sdk\patina\src\component.rs @ 165                    ┆ 000000007E8D47B0 ┆ 000000007E915B3E ┆ qemu_q35_dxe_core!patina::component::Component::run<patina::compone │
│    ┆                                                                      ┆                  ┆                  ┆ nt::struct_component::StructComponent<tuple$<patina::component::par │
│    ┆                                                                      ┆                  ┆                  ┆ ams::RunOnce,enum2$<core::result::Result<tuple$<>,enum2$<patina::er │
│    ┆                                                                      ┆                  ┆                  ┆ ror::EfiError> > > (*)(patina_samples::component::hello_world::Hell │
│    ┆                                                                      ┆                  ┆                  ┆ oStruct,patina::component::params::Config<i32>)>,enum2$<core::resul │
│    ┆                                                                      ┆                  ┆                  ┆ t::Result<tuple$<>,enum2$<patina::error::EfiError> > > (*)(patina_s │
│    ┆                                                                      ┆                  ┆                  ┆ amples::component::hello_world::HelloStruct,patina::component::para │
│    ┆                                                                      ┆                  ┆                  ┆ ms::Config<i32>)> >(patina::component::struct_component::StructComp │
│    ┆                                                                      ┆                  ┆                  ┆ onent<tuple$<patina::component::params::RunOnce,enum2$<core::result │
│    ┆                                                                      ┆                  ┆                  ┆ ::Result<tuple$<>,enum2$<patina::error::EfiError> > > (*)(patina_sa │
│    ┆                                                                      ┆                  ┆                  ┆ mples::component::hello_world::HelloStruct,patina::component::param │
│    ┆                                                                      ┆                  ┆                  ┆ s::Config<i32>)>,enum2$<core::result::Result<tuple$<>,enum2$<patina │
│    ┆                                                                      ┆                  ┆                  ┆ ::error::EfiError> > > (*)(patina_samples::component::hello_world:: │
│    ┆                                                                      ┆                  ┆                  ┆ HelloStruct,patina::component::params::Config<i32>)>*,              │
│    ┆                                                                      ┆                  ┆                  ┆ patina::component::storage::Storage*)+0x2E                          │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 7  ┆ E:\repos\patina\patina_dxe_core\src\lib.rs @ 371                     ┆ 000000007E8D47F0 ┆ 000000007E9114C8 ┆ qemu_q35_dxe_core!patina_dxe_core::impl$4::dispatch_components::clo │
│    ┆                                                                      ┆                  ┆                  ┆ sure$0(patina_dxe_core::impl$4::dispatch_components::closure_env$0* │
│    ┆                                                                      ┆                  ┆                  ┆ , alloc::boxed::Box<dyn$<patina::component::Component>,alloc::alloc │
│    ┆                                                                      ┆                  ┆                  ┆ ::Global>*)+0xDA                                                    │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 8  ┆ C:\Users\vineelko\.rustup\toolchains\1.89.0-x86_64-pc-windows-msvc\l ┆ 000000007E8D48F0 ┆ 000000007E911D81 ┆ qemu_q35_dxe_core!alloc::vec::impl$1::retain_mut::process_loop<pati │
│    ┆ ib\rustlib\src\rust\library\alloc\src\vec\mod.rs @ 2243              ┆                  ┆                  ┆ na_dxe_core::impl$4::dispatch_components::closure_env$0,alloc::boxe │
│    ┆                                                                      ┆                  ┆                  ┆ d::Box<dyn$<patina::component::Component>,alloc::alloc::Global>,all │
│    ┆                                                                      ┆                  ┆                  ┆ oc::alloc::Global,true>(unsigned long long,                         │
│    ┆                                                                      ┆                  ┆                  ┆ patina_dxe_core::impl$4::dispatch_components::closure_env$0*, alloc │
│    ┆                                                                      ┆                  ┆                  ┆ ::vec::impl$1::retain_mut::BackshiftOnDrop<alloc::boxed::Box<dyn$<p │
│    ┆                                                                      ┆                  ┆                  ┆ atina::component::Component>,alloc::alloc::Global>,alloc::alloc::Gl │
│    ┆                                                                      ┆                  ┆                  ┆ obal>*)+0x68                                                        │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 9  ┆ C:\Users\vineelko\.rustup\toolchains\1.89.0-x86_64-pc-windows-msvc\l ┆ 000000007E8D49A0 ┆ 000000007E8EB7AE ┆ qemu_q35_dxe_core!alloc::vec::Vec<alloc::boxed::Box<dyn$<patina::co │
│    ┆ ib\rustlib\src\rust\library\alloc\src\vec\mod.rs @ 2275              ┆                  ┆                  ┆ mponent::Component>,alloc::alloc::Global>,alloc::alloc::Global>::re │
│    ┆                                                                      ┆                  ┆                  ┆ tain_mut<alloc::boxed::Box<dyn$<patina::component::Component>,alloc │
│    ┆                                                                      ┆                  ┆                  ┆ ::alloc::Global>,alloc::alloc::Global,patina_dxe_core::impl$4::disp │
│    ┆                                                                      ┆                  ┆                  ┆ atch_components::closure_env$0>(patina_dxe_core::impl$4::dispatch_c │
│    ┆                                                                      ┆                  ┆                  ┆ omponents::closure_env$0)+0x50                                      │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 10 ┆ E:\repos\patina\patina_dxe_core\src\lib.rs @ 398                     ┆ 000000007E8D4A10 ┆ 000000007E8EC7F6 ┆ qemu_q35_dxe_core!patina_dxe_core::Core<patina_dxe_core::Alloc>::co │
│    ┆                                                                      ┆                  ┆                  ┆ re_dispatcher()+0x89                                                │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 11 ┆ E:\repos\patina\patina_dxe_core\src\lib.rs @ 555                     ┆ 000000007E8D4B10 ┆ 000000007E8DCA9D ┆ qemu_q35_dxe_core!patina_dxe_core::Core<patina_dxe_core::Alloc>::st │
│    ┆                                                                      ┆                  ┆                  ┆ art(patina_dxe_core::Core<patina_dxe_core::Alloc>)+0xEAE            │
├╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ 12 ┆ E:\repos\patina-dxe-core-qemu\bin\q35_dxe_core.rs @ 113              ┆ 000000007E8D4D10 ┆ 000000007EBE338F ┆ qemu_q35_dxe_core!qemu_q35_dxe_core::_start(core::ffi::c_void*)+0x4 │
│    ┆                                                                      ┆                  ┆                  ┆ 14                                                                  │
└────┴──────────────────────────────────────────────────────────────────────┴──────────────────┴──────────────────┴─────────────────────────────────────────────────────────────────────┘
# cspell:enable
```

The input may include additional whitespace before each frame, a timestamp in
the format shown, or a log prefix level. All of those are ignored.

## Allowed Examples

Each of these examples will produce the same output:

```cmd
13:39:09.014 : INFO -       # Child-SP              Return Address         Call Site
13:39:09.014 : INFO -       0 000000007E96F930      000000007E982668       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+6E1BF
13:39:09.014 : INFO -       1 000000007E96F960      000000007EA8B92F       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+12668
13:39:09.014 : INFO -       2 000000007E96FA70      000000007E9739E0       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+11B92F
13:39:09.014 : INFO -       3 000000007E96FAB0      000000007E98301D       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+39E0
13:39:09.014 : INFO -       4 000000007E96FC00      000000007EBE62F4       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+1301D
```

```cmd
INFO -       # Child-SP              Return Address         Call Site
INFO -       0 000000007E96F930      000000007E982668       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+6E1BF
INFO -       1 000000007E96F960      000000007EA8B92F       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+12668
INFO -       2 000000007E96FA70      000000007E9739E0       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+11B92F
INFO -       3 000000007E96FAB0      000000007E98301D       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+39E0
INFO -       4 000000007E96FC00      000000007EBE62F4       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+1301D
```

```cmd
# Child-SP              Return Address         Call Site
0 000000007E96F930      000000007E982668       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+6E1BF
1 000000007E96F960      000000007EA8B92F       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+12668
2 000000007E96FA70      000000007E9739E0       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+11B92F
3 000000007E96FAB0      000000007E98301D       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+39E0
4 000000007E96FC00      000000007EBE62F4       qemu_q35_dxe_core-2d9bed3cc1f2b4ea+1301D
```

![Stack Trace Diagram](stacktrace.png)

## Prerequisites

This library uses the PE image `.pdata` section to calculate the stack unwind
information required to walk the call stack. Therefore, compile all binaries
with the following `rustc` flag to generate the `.pdata` section in the PE
images:

`RUSTFLAGS=-Cforce-unwind-tables`

To preserve stack data for C binaries, set the following flag in the platform
DSC build-options section:

`*_*_*_GENFW_FLAGS   = --keepexceptiontable`

## Supported Platforms

- Hardware
  - X64
  - AArch64

- File Formats
  - PE32+

- Environments
  - UEFI
  - Windows
  - Linux

## Public API

The primary public API is the `dump()` function in the `StackTrace` module.

```rust
    /// Dumps the stack trace for the given PC, SP, and FP values.
    ///
    /// # Safety
    ///
    /// This function is marked `unsafe` to indicate that the caller is
    /// responsible for validating the provided PC, SP, and FP values. Invalid
    /// values can result in undefined behavior, including potential page
    /// faults.
    ///
    /// ```text
    /// # Child-SP              Return Address         Call Site
    /// 0 0000005E2AEFFC00      00007FFB10CB4508       aarch64+44B0
    /// 1 0000005E2AEFFC20      00007FFB10CB45A0       aarch64+4508
    /// 2 0000005E2AEFFC40      00007FFB10CB4640       aarch64+45A0
    /// 3 0000005E2AEFFC60      00007FFB10CB46D4       aarch64+4640
    /// 4 0000005E2AEFFC90      00007FF760473B98       aarch64+46D4
    /// 5 0000005E2AEFFCB0      00007FFB8F062310       patina_stacktrace-45f5092641a5979a+3B98
    /// 6 0000005E2AEFFD10      00007FFB8FF95AEC       kernel32+12310
    /// 7 0000005E2AEFFD50      0000000000000000       ntdll+75AEC
    /// ```
    pub unsafe fn dump_with(stack_frame: StackFrame) -> StResult<()>;

    /// Dumps the stack trace. This function reads the PC, SP, and FP values and
    /// attempts to dump the call stack.
    ///
    /// # Safety
    ///
    /// It is marked `unsafe` to indicate that the caller is responsible for the
    /// validity of the PC, SP, and FP values. Invalid or corrupt machine state
    /// can result in undefined behavior, including potential page faults.
    ///
    /// ```text
    /// # Child-SP              Return Address         Call Site
    /// 0 0000005E2AEFFC00      00007FFB10CB4508       aarch64+44B0
    /// 1 0000005E2AEFFC20      00007FFB10CB45A0       aarch64+4508
    /// 2 0000005E2AEFFC40      00007FFB10CB4640       aarch64+45A0
    /// 3 0000005E2AEFFC60      00007FFB10CB46D4       aarch64+4640
    /// 4 0000005E2AEFFC90      00007FF760473B98       aarch64+46D4
    /// 5 0000005E2AEFFCB0      00007FFB8F062310       patina_stacktrace-45f5092641a5979a+3B98
    /// 6 0000005E2AEFFD10      00007FFB8FF95AEC       kernel32+12310
    /// 7 0000005E2AEFFD50      0000000000000000       ntdll+75AEC
    /// ```
    pub unsafe fn dump() -> StResult<()>;
```

## API usage

```rust
  // Inside an exception handler
    let stack_frame = StackFrame { pc: x64_context.rip, sp: x64_context.rsp, fp: x64_context.rbp };
    StackTrace::dump_with(stack_frame); // X64
    let stack_frame = StackFrame { pc: aarch64_context.elr, sp: aarch64_context.sp, fp: aarch64_context.fp };
  StackTrace::dump_with(stack_frame); // AArch64

  // Inside a Rust panic handler and drivers
    StackTrace::dump();
```

## Reference

More reference test cases are available in `src\x64\tests\*.rs`.
