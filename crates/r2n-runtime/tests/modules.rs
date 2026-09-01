//! M2-T09 runtime acceptance: a multi-module program linked into one flat table
//! actually renders the imported component, and a module namespace bound for
//! dynamic `import("...")` is readable as a `ComponentRefVal` value.

use r2n_compiler::{link_source, MemResolver};
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn resolver(modules: &[(&str, &str)]) -> MemResolver {
    let mut r = MemResolver::new();
    for (id, src) in modules {
        r.add(*id, *src);
    }
    r
}

/// Link `modules`, run the runtime, and return the rendered markup.
fn render(modules: &[(&str, &str)]) -> String {
    let entry = &modules[0];
    let r = resolver(modules);
    let template = link_source(entry.1, entry.0, &r).expect("link");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut renderer = MemoryRenderer::new();
    renderer.apply(&patches);
    renderer.render_string()
}

#[test]
fn renders_an_imported_component_across_modules() {
    let entry = r#"
        import { Widget } from "widget";
        component App() {
            return <div className="app"><Widget/></div>;
        }
        export default App;
    "#;
    let widget = r#"
        component Widget() {
            return <span className="w">hi</span>;
        }
        export { Widget };
    "#;
    let out = render(&[("app", entry), ("widget", widget)]);
    assert_eq!(
        out,
        "<div className=\"app\"><span className=\"w\">hi</span></div>",
        "imported component renders inside the entry tree"
    );
}

#[test]
fn dynamic_import_reads_a_module_namespace() {
    // The module is statically imported (so it is linked + bound); a dynamic
    // `import("widget")` then resolves to the SAME canonical namespace, exposing
    // its exported component as a ComponentRefVal value that reads as its handle.
    let entry = r#"
        import { Widget } from "widget";
        component App() {
            let m = import("widget");
            return <div>{m.Widget}</div>;
        }
        export default App;
    "#;
    let widget = r#"
        component Widget() {
            return <span className="w">hi</span>;
        }
        export { Widget };
    "#;
    let out = render(&[("app", entry), ("widget", widget)]);
    assert_eq!(
        out,
        "<div><component#1></div>",
        "namespace-held component reads as its ComponentRefVal handle"
    );
}
#[test]
fn dynamically_only_imported_module_renders_its_component_ref() {
    // `lazy` is reachable only via `import("./lazy")` — never statically
    // imported. The linker must discover it, and the relative specifier must be
    // canonicalized so the runtime's `@module:lazy` namespace resolves.
    let entry = r#"
        component App() {
            const m = import("./lazy");
            return <div>{m.Lazy}</div>;
        }
        export default App;
    "#;
    let lazy = r#"
        component Lazy() { return <span className="l">z</span>; }
        export { Lazy };
    "#;
    let out = render(&[("app", entry), ("lazy", lazy)]);
    assert_eq!(
        out,
        "<div><component#1></div>",
        "dynamically-only module component reads as a ComponentRefVal"
    );
}

