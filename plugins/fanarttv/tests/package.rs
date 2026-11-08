#![cfg(not(target_arch = "wasm32"))]
//! The provider packages and verifies. Needs the release component
//! (`cargo build -p fk-plugin-fanarttv --release --target wasm32-wasip2`).

use fliks_plugin_sdk::build::{self, Crate};
use std::path::Path;

#[test]
fn the_provider_packages_and_verifies() {
    let c = Crate::at(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let (name, archive) = c.package(None).unwrap();
    assert_eq!(
        name,
        format!("fliks.fanarttv-{}.fkplugin", env!("CARGO_PKG_VERSION"))
    );
    let i = build::verify(&archive, vec![], build::MAX_WASM).unwrap();
    let p = &i.manifest["provider"];
    assert_eq!(p["namespaces"][0]["name"], "fanarttv");
    assert_eq!(
        p["kinds"]["album"],
        serde_json::json!(["work", "person", "findBy"])
    );
    assert_eq!(p["requires"], serde_json::json!(["apiKey"]));
    let wasm = std::fs::read(c.wasm_path()).unwrap();
    build::check_world(&wasm, true).unwrap();
}
