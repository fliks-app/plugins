#![cfg(not(target_arch = "wasm32"))]
//! The plugin packages and verifies. Needs the release component
//! (`cargo build -p fk-plugin-music-lists --release --target wasm32-wasip2`).

use fliks_plugin_sdk::build::{self, Crate};
use std::path::Path;

#[test]
fn the_plugin_packages_and_verifies() {
    let c = Crate::at(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let (name, archive) = c.package(None).unwrap();
    assert_eq!(
        name,
        format!("fliks.musiclists-{}.fkplugin", env!("CARGO_PKG_VERSION"))
    );
    let i = build::verify(&archive, vec![], build::MAX_WASM).unwrap();
    assert_eq!(
        i.manifest["scopes"],
        serde_json::json!(["media:read", "media:create", "playlists:write"])
    );
    let wasm = std::fs::read(c.wasm_path()).unwrap();
    build::check_world(&wasm, false).unwrap();
}
