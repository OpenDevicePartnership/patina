use crate::{
    pe::PE,
    x64::{
        runtime_function::RuntimeFunction,
        tests::{module::Module, set_logger},
    },
};
#[test]
fn test_unwind_info() {
    set_logger();
    let path = r"src\x64\tests\collateral\x64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    unsafe {
        let image = PE::locate_image(module.base_address);
        assert!(image.is_ok());

        let image = image.unwrap();

        let runtime_functions = RuntimeFunction::find_all_functions(&image);
        assert!(runtime_functions.is_ok());

        let runtime_functions = runtime_functions.unwrap();
        println!("runtime_functions: ");
        for func in &runtime_functions {
            println!("Function Name: {}", func);

            let unwind_info = func.get_unwind_info().unwrap();
            println!("unwind_info: {}", unwind_info);

            let rsp_offset = unwind_info.get_stack_pointer_offset().unwrap();
            println!("rsp_offset: {} {:X}", rsp_offset, rsp_offset);
        }
    }

    module.unload();
}
