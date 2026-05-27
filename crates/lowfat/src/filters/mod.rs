//! Native filter registry — historically held git/docker/ls compiled in
//! Rust. Those are now bundled `.lf` plugins (see `crates/lowfat-plugin/src/
//! embedded.rs`), so this returns an empty map. The function is kept as the
//! single hook for any future Rust-coded coreutils filters; current call
//! sites pass through it unchanged.

use lowfat_plugin::plugin::FilterPlugin;
use std::collections::HashMap;

pub fn builtins() -> HashMap<String, Box<dyn FilterPlugin>> {
    HashMap::new()
}
