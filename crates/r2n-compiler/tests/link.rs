//! M2-T09 linker tests: assembling a multi-module program into one flat global
//! component table, with cross-module `import`/`export` resolution, dedup,
//! cycle detection, and deterministic global index assignment.

use r2n_compiler::{link_source, MemResolver};

/// Build an in-memory resolver with the given `id -> source` modules.
fn resolver(modules: &[(&str, &str)]) -> MemResolver {
    let mut r = MemResolver::new();
    for (id, src) in modules {
        r.add(*id, *src);
    }
    r
}

#[test]
fn imports_a_component_across_modules_into_one_global_table() {
    let entry = r#"
        import { Widget } from "widget";
        component App() {
            return <div><Widget/></div>;
        }
        export default App;
    "#;
    let widget = r#"
        component Widget() {
            return <span>w</span>;
        }
        export { Widget };
    "#;
    let r = resolver(&[("app", entry), ("widget", widget)]);
    let t = link_source(entry, "app", &r).expect("link");

    // Both modules flatten into one table; entry first (index 0).
    assert_eq!(t.components.len(), 2, "app + widget");
    assert_eq!(t.components[0].name, "App");
    assert_eq!(t.components[1].name, "Widget");
    assert_eq!(t.root, 0, "entry default is the root");

    // App's body is a <div> whose lone child is the imported <Widget/> — lowered
    // to a compile-time ComponentRef into the GLOBAL table (index 1).
    match &t.components[0].body {
        r2n_ir::react::ReactNode::Host { children, .. } => match &children[0] {
            r2n_ir::react::ReactNode::Component { component, .. } => {
                assert_eq!(component.index(), 1, "imported component resolves globally");
            }
            other => panic!("expected imported component child, got {other:?}"),
        },
        other => panic!("expected host root, got {other:?}"),
    }

    // Both modules register namespaces for dynamic import.
    assert_eq!(t.modules.len(), 2);
    assert_eq!(t.modules[0].id, "app");
    assert_eq!(t.modules[0].exports, vec![("default".to_string(), 0)]);
    assert_eq!(t.modules[1].id, "widget");
    assert_eq!(t.modules[1].exports, vec![("Widget".to_string(), 1)]);
}

#[test]
fn named_import_alias_resolves_to_the_export() {
    let entry = r#"
        import { Widget as W } from "widget";
        component App() {
            return <div><W/></div>;
        }
        export default App;
    "#;
    let widget = r#"
        component Widget() { return <span>w</span>; }
        export { Widget };
    "#;
    let r = resolver(&[("app", entry), ("widget", widget)]);
    let t = link_source(entry, "app", &r).expect("link");
    match &t.components[0].body {
        r2n_ir::react::ReactNode::Host { children, .. } => match &children[0] {
            r2n_ir::react::ReactNode::Component { component, .. } => {
                assert_eq!(component.index(), 1);
            }
            other => panic!("expected component child, got {other:?}"),
        },
        other => panic!("expected host root, got {other:?}"),
    }
}

#[test]
fn diamond_imports_are_deduplicated_to_one_shared_module() {
    let entry = r#"
        import A from "a";
        import B from "b";
        component App() {
            return <div><A/><B/></div>;
        }
        export default App;
    "#;
    let a = r#"
        component A() { return <div/>; }
        export default A;
        import C from "c";
    "#;
    let b = r#"
        component B() { return <div/>; }
        export default B;
        import C from "c";
    "#;
    let c = r#"
        component C() { return <div/>; }
        export default C;
    "#;
    let r = resolver(&[("app", entry), ("a", a), ("b", b), ("c", c)]);
    let t = link_source(entry, "app", &r).expect("link");

    // app, a, c (deduped), b — C appears exactly once in the table.
    let names: Vec<&str> = t.components.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(t.components.len(), 4, "App, A, C, B deduped: {names:?}");
    assert_eq!(names.iter().filter(|n| **n == "C").count(), 1);
    assert_eq!(names[2], "C", "C is linked once, in discovery order");
}

#[test]
fn import_cycle_is_a_precise_error() {
    let entry = r#"
        import A from "a";
        component App() { return <div><A/></div>; }
        export default App;
    "#;
    let a = r#"
        component A() { return <div/>; }
        export default A;
        import B from "b";
    "#;
    let b = r#"
        component B() { return <div/>; }
        export default B;
        import A from "a";
    "#;
    let r = resolver(&[("app", entry), ("a", a), ("b", b)]);
    let err = link_source(entry, "app", &r).unwrap_err();
    assert!(
        matches!(err, r2n_compiler::LinkError::ImportCycle(_)),
        "got: {err}"
    );
}

#[test]
fn unknown_export_is_a_precise_error() {
    let entry = r#"
        import { Nope } from "widget";
        component App() { return <div/>; }
        export default App;
    "#;
    let widget = "component Widget() { return <span/>; }\nexport { Widget };";
    let r = resolver(&[("app", entry), ("widget", widget)]);
    let err = link_source(entry, "app", &r).unwrap_err();
    assert!(
        matches!(err, r2n_compiler::LinkError::UnknownExport { .. }),
        "got: {err}"
    );
}

#[test]
fn entry_without_default_uses_its_sole_component() {
    // No `export default`, but the entry declares exactly ONE component:
    // that component is the root (the `export function App()` shape real
    // React apps use). The imported module still needs no default.
    let entry = r#"
        import { Widget } from "widget";
        component App() { return <div><Widget/></div>; }
    "#;
    let widget = r#"
        component Widget() { return <span/>; }
        export { Widget };
    "#;
    let r = resolver(&[("app", entry), ("widget", widget)]);
    let t = link_source(entry, "app", &r).expect("sole entry component is the root");
    assert_eq!(t.root, 0);
    assert_eq!(t.components[0].name, "App");
}

#[test]
fn entry_with_several_components_still_needs_a_default() {
    // Ambiguous entry (two components, no default) is still a precise
    // NoDefault error — the fallback only applies when there is exactly one.
    let entry = r#"
        component App() { return <div/>; }
        component Other() { return <span/>; }
    "#;
    let r = resolver(&[("app", entry)]);
    let err = link_source(entry, "app", &r).unwrap_err();
    assert!(
        matches!(err, r2n_compiler::LinkError::NoDefault(_)),
        "got: {err}"
    );
}

#[test]
fn default_import_binds_the_targets_default_export() {
    let entry = r#"
        import Widget from "widget";
        component App() {
            return <div><Widget/></div>;
        }
        export default App;
    "#;
    let widget = r#"
        component Widget() { return <span>w</span>; }
        export default Widget;
    "#;
    let r = resolver(&[("app", entry), ("widget", widget)]);
    let t = link_source(entry, "app", &r).expect("link");
    match &t.components[0].body {
        r2n_ir::react::ReactNode::Host { children, .. } => match &children[0] {
            r2n_ir::react::ReactNode::Component { component, .. } => {
                assert_eq!(component.index(), 1);
            }
            other => panic!("expected component child, got {other:?}"),
        },
        other => panic!("expected host root, got {other:?}"),
    }
}
#[test]
fn dynamically_imported_module_is_discovered_and_linked() {
    // `lazy` is imported ONLY via `import("lazy")` inside App's body — it is
    // never statically imported, so the linker's DFS must walk the AST to find
    // it, lay it out, and bind its namespace.
    let entry = r#"
        component App() {
            const m = import("lazy");
            return <div>{m.Lazy}</div>;
        }
        export default App;
    "#;
    let lazy = r#"
        component Lazy() { return <span>lazy</span>; }
        export { Lazy };
    "#;
    let r = resolver(&[("app", entry), ("lazy", lazy)]);
    let t = link_source(entry, "app", &r).expect("link");

    // Both modules are linked; discovery order is app then lazy.
    assert_eq!(t.components.len(), 2, "App + dynamically-imported Lazy");
    assert_eq!(t.components[1].name, "Lazy");
    assert_eq!(t.modules.len(), 2);
    assert!(
        t.modules.iter().any(|m| m.id == "lazy"),
        "dynamically-reachable module is bound as a namespace"
    );
}

#[test]
fn dynamic_import_specifier_is_canonicalized_to_the_resolved_id() {
    // A dynamic `import("./widget")` must be rewritten to the canonical id
    // ("widget") so the lowerer's `@module:` key matches the runtime binding.
    let entry = r#"
        component App() {
            const m = import("./widget");
            return <div>{m.Widget}</div>;
        }
        export default App;
    "#;
    let widget = r#"
        component Widget() { return <span>w</span>; }
        export { Widget };
    "#;
    let r = resolver(&[("app", entry), ("widget", widget)]);
    let t = link_source(entry, "app", &r).expect("link");

    // The namespace is bound under the canonical id.
    assert!(t.modules.iter().any(|m| m.id == "widget"));
    // The binding's value is the canonical `@module:widget`, proving the raw
    // "./widget" was rewritten before lowering.
    let root = &t.components[0];
    assert_eq!(root.bindings.len(), 1);
    assert!(
        matches!(&root.bindings[0].1, r2n_ir::js::JsExpr::Var(v) if v == "@module:widget"),
        "dynamic import specifier canonicalized"
    );
}
