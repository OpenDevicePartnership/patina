use std::{
    env,
    path::{Path, PathBuf},
};

pub(crate) fn project_root() -> PathBuf {
    Path::new(&env!("CARGO_MANIFEST_DIR")).ancestors().nth(1).unwrap().to_path_buf()
}

pub(crate) fn project_target_dir() -> PathBuf {
    let mut target_dir = project_root();
    target_dir.push("target");
    target_dir
}

pub(crate) fn cargo_bin_dir() -> PathBuf {
    let mut path = env!("CARGO_HOME").to_string();
    path.push_str("bin");
    PathBuf::from(path)
}
