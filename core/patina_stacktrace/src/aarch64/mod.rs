pub(crate) mod runtime_function;
mod unwind;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "windows", target_arch = "aarch64", test))] {
        #[coverage(off)]
        pub mod tests;
    }
}
