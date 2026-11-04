#![cfg(not(target_arch = "wasm32"))]
//! The provider packages and verifies. Needs the release component
//! (`cargo build -p fk-plugin-metron --release --target wasm32-wasip2`).

use fliks_plugin_sdk::build::{self, Crate};
use std::path::Path;

#[test]
fn the_provider_packages_and_verifies() {
    let c = Crate::at(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let (name, archive) = c.package(None).unwrap();
    assert_eq!(
        name,
        format!("fliks.metron-{}.fkplugin", env!("CARGO_PKG_VERSION"))
    );
    let i = build::verify(&archive, vec![], build::MAX_WASM).unwrap();
    let ns = &i.manifest["provider"]["namespaces"][0];
    assert_eq!(ns["name"], "fliks.metron:metron");
    assert_eq!(
        i.manifest["provider"]["requires"],
        serde_json::json!(["apiToken"])
    );
    let wasm = std::fs::read(c.wasm_path()).unwrap();
    build::check_world(&wasm, true).unwrap();
}
