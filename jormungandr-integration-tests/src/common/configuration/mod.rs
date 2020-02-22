#![allow(dead_code)]

extern crate lazy_static;
extern crate rand;

use self::lazy_static::lazy_static;
use self::rand::Rng;
use super::file_utils;
use escargot::CargoBuild;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};

mod block0_config_builder;
pub mod jormungandr_config;
mod node_config_builder;
mod secret_model_factory;

pub use block0_config_builder::Block0ConfigurationBuilder;
pub use jormungandr_config::JormungandrConfig;
pub use node_config_builder::NodeConfigBuilder;
pub use secret_model_factory::SecretModelFactory;

lazy_static! {
    static ref JORMUNGANDR_BIN_PATH: PathBuf = {
        CargoBuild::new()
            .package("jormungandr")
            .bin("jormungandr")
            .current_release()
            .run()
            .unwrap()
            .path()
            .into()
    };
    static ref JCLI_BIN_PATH: PathBuf = {
        CargoBuild::new()
            .package("jcli")
            .bin("jcli")
            .current_release()
            .run()
            .unwrap()
            .path()
            .into()
    };
}

/// Get jormungandr executable from current environment
pub fn get_jormungandr_app() -> &'static Path {
    &JORMUNGANDR_BIN_PATH
}

/// Get jcli executable from current environment
pub fn get_jcli_app() -> &'static Path {
    &JCLI_BIN_PATH
}

fn get_workspace_directory() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let has_parent = path.pop();
    assert!(has_parent);
    path
}

/// Gets working directory
/// Uses std::env::current_exe() for this purpose.
/// Current exe directory is ./target/{profile}/deps/{app_name}.exe
/// Function returns ./target/{profile}
fn get_working_directory() -> PathBuf {
    let mut output_directory: PathBuf = std::env::current_exe().unwrap().into();

    output_directory.pop();
    output_directory.pop();
    output_directory
}

pub fn get_openapi_path() -> PathBuf {
    let mut path = get_workspace_directory();
    path.push("doc");
    path.push("openapi.yaml");
    path
}

lazy_static! {
    static ref NEXT_AVAILABLE_PORT_NUMBER: AtomicU16 = {
        let initial_port = rand::thread_rng().gen_range(6000, 10999);
        AtomicU16::new(initial_port)
    };
}

pub fn get_available_port() -> u16 {
    NEXT_AVAILABLE_PORT_NUMBER.fetch_add(1, Ordering::SeqCst)
}
