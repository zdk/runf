//! Plugins bundled into the binary at compile time.
//!
//! These are the canonical replacements for the deleted native Rust filters
//! (git/docker/ls). They ship as **data** — DSL configuration, not Rust code —
//! so the lowfat binary itself only contains coreutils-equivalent logic + the
//! plugin protocol. A user can shadow any bundled plugin by dropping a file
//! at `~/.lowfat/plugins/<category>/<name>/filter.lf` — disk wins over bundled
//! in `discover_plugins`.
//!
//! Only the load-bearing files (`lowfat.toml` + `filter.lf`) are embedded.
//! Samples, BENCHMARK.md, bench.sh, and the legacy filter.sh are deliberately
//! left out of the binary — they're documentation, not runtime.

pub struct EmbeddedPlugin {
    pub category: &'static str,
    pub name: &'static str,
    pub manifest: &'static str,
    pub filter_lf: &'static str,
}

pub const EMBEDDED: &[EmbeddedPlugin] = &[
    EmbeddedPlugin {
        category: "git",
        name: "git-compact",
        manifest: include_str!("../../../plugins/git/git-compact/lowfat.toml"),
        filter_lf: include_str!("../../../plugins/git/git-compact/filter.lf"),
    },
    EmbeddedPlugin {
        category: "docker",
        name: "docker-compact",
        manifest: include_str!("../../../plugins/docker/docker-compact/lowfat.toml"),
        filter_lf: include_str!("../../../plugins/docker/docker-compact/filter.lf"),
    },
    EmbeddedPlugin {
        category: "ls",
        name: "ls-compact",
        manifest: include_str!("../../../plugins/ls/ls-compact/lowfat.toml"),
        filter_lf: include_str!("../../../plugins/ls/ls-compact/filter.lf"),
    },
];
