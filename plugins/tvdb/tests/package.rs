#![cfg(not(target_arch = "wasm32"))]
//! The provider component packages and verifies. Needs the release component
//! (`cargo build --release --target wasm32-wasip2`).

use fliks_plugin_sdk::build::{self, Crate};
use std::path::Path;

#[test]
fn the_provider_packages_and_verifies() {
    let c = Crate::at(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let (name, archive) = c.package(None).unwrap();
    assert_eq!(name, "fliks.tvdb-0.1.0.fkplugin");
    let i = build::verify(&archive, vec![], build::MAX_WASM).unwrap();
    assert_eq!(i.manifest["provider"]["namespaces"][0]["name"], "tvdb");
    let wasm = std::fs::read(c.wasm_path()).unwrap();
    build::check_world(&wasm, true).unwrap();
}
