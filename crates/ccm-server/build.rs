//! Points `CCM_UI_DIR` at the built web UI (`dist/` from `pnpm build`) for
//! rust-embed, or at a placeholder page when it hasn't been built, so the
//! server compiles either way.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let dir = if dist.join("index.html").is_file() {
        dist.canonicalize().unwrap()
    } else {
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ui-placeholder");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(
            out.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>CC Manager</title>\
             <p style=\"font:16px system-ui;margin:3em\">Web UI not built into this binary. \
             Run <code>pnpm install &amp;&amp; pnpm build</code>, then rebuild ccm-server. \
             The API under <code>/admin/api</code> works regardless.</p>",
        )
        .unwrap();
        out
    };
    println!("cargo:rustc-env=CCM_UI_DIR={}", dir.display());
}
