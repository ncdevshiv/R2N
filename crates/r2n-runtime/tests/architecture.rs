//! Architecture guard (M0.1 P2): the runtime must NEVER depend on the
//! parser, AST, or compiler crates. It executes compiled Runtime IR only —
//! that is the locked boundary rule ("the runtime never sees source code").
//! This test reads the runtime's manifest so the rule is enforced by CI, not
//! by memory.

#[test]
fn runtime_never_depends_on_parser_ast_or_compiler() {
    let manifest = include_str!("../Cargo.toml");
    let dep_section = manifest
        .split("[dependencies]")
        .nth(1)
        .and_then(|s| s.split("[dev-dependencies]").next())
        .expect("runtime manifest must have a [dependencies] section");
    for forbidden in ["r2n-parser", "r2n-ast", "r2n-compiler"] {
        assert!(
            !dep_section.contains(forbidden),
            "ARCHITECTURE VIOLATION: r2n-runtime depends on {forbidden}; \
             the runtime must consume only compiled Runtime IR (see \
             roadmap/ROADMAP.md boundary rules)"
        );
    }
}

#[test]
fn renderer_never_depends_on_compiler() {
    let manifest = include_str!("../../r2n-renderer-memory/Cargo.toml");
    let dep_section = manifest
        .split("[dependencies]")
        .nth(1)
        .and_then(|s| s.split("[dev-dependencies]").next())
        .expect("renderer manifest must have a [dependencies] section");
    assert!(
        !dep_section.contains("r2n-compiler"),
        "ARCHITECTURE VIOLATION: r2n-renderer-memory depends on r2n-compiler \
         outside dev-dependencies; renderers consume only the Patch stream"
    );
}
