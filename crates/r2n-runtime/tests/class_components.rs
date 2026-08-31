//! M1-T12 acceptance: class components.
//!
//! React semantics under test:
//! 1. `class X extends Component { state = ...; render() {...} }` renders.
//! 2. `this.state` reads the instance state; `this.setState(v)` updates it
//!    and re-renders; this inside a method is the instance.
//! 3. Two instances have independent state.
//! 4. Lifecycle: componentDidMount once after mount, componentDidUpdate on
//!    re-renders, componentWillUnmount once at unmount.
//! 5. Class components compose with function components (parent uses one).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{NodeId, Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

fn clickable(r: &MemoryRenderer) -> NodeId {
    r.nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, props } => {
                if tag == "button"
                    && props.iter().any(|(k, v)| {
                        k == "onClick" && matches!(v, r2n_runtime::Value::Handler { .. })
                    })
                {
                    Some(*id)
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("clickable button")
}

#[test]
fn class_renders_state_and_updates_via_setstate() {
    let src = r#"
        class Counter extends Component {
            state = 0;
            inc() { this.setState(this.state + 1); }
            render() {
                return (
                    <div>
                        <p className="count">{this.state}</p>
                        <button onClick={() => this.inc()}>+</button>
                    </div>
                );
            }
        }
        export default Counter;
    "#;
    let (mut rt, mut r) = setup(src);
    let tree = r.render_string();
    assert!(tree.contains(">0<"), "initial state: {tree}");
    let btn = clickable(&r);
    let patches = rt.dispatch(btn, "onClick").expect("inc");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">1<"),
        "after setState: {}",
        r.render_string()
    );
    let set_text = patches
        .iter()
        .any(|p| matches!(p, r2n_runtime::Patch::SetText { .. }));
    let removals = patches
        .iter()
        .filter(|p| matches!(p, r2n_runtime::Patch::Remove { .. }))
        .count();
    assert!(set_text, "minimal SetText: {patches:?}");
    assert_eq!(removals, 0, "no recreation: {patches:?}");
}

#[test]
fn class_instances_independent() {
    let src = r#"
        class Counter extends Component {
            state = 0;
            inc() { this.setState(this.state + 1); }
            render() { return <div className="c"><span>{this.state}</span><button onClick={() => this.inc()}>+</button></div>; }
        }
        component App() {
            return <main><Counter/><Counter/></main>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = clickable(&r);
    let patches = rt.dispatch(btn, "onClick").expect("first inc");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree.contains(">1<") && tree.contains(">0<"),
        "one advanced, one not: {tree}"
    );
}

#[test]
fn class_lifecycle_methods_fire() {
    let src = r#"
        class Widget extends Component {
            state = 0;
            componentDidMount() { log("did-mount"); }
            componentDidUpdate() { log("did-update"); }
            componentWillUnmount() { log("will-unmount"); }
            render() {
                return <div><p className="w">{this.state}</p><button onClick={() => this.setState(1)}>go</button></div>;
            }
        }
        component App() {
            let v = useState(0);
            let on = v[0];
            let setOn = v[1];
            return <div>{on % 2 == 0 && <Widget/>}<button className="flip" onClick={() => setOn(on + 1)}>flip</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let logs = rt.logs();
    assert!(
        logs.contains(&"did-mount".to_string()),
        "mount fired: {logs:?}"
    );
    // Find the go button (first) and click it: state change -> didUpdate.
    let go = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, props } => {
                if tag == "button"
                    && !props.iter().any(|(k, _)| k == "className")
                    && props.iter().any(|(k, v)| {
                        k == "onClick" && matches!(v, r2n_runtime::Value::Handler { .. })
                    })
                {
                    Some(*id)
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("go button");
    let patches = rt.dispatch(go, "onClick").expect("setState");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.contains(&"did-update".to_string()),
        "didUpdate after setState: {logs:?}"
    );
    // Unmount: flip togger (className=flip).
    let flip = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, props } => {
                if tag == "button" && props.iter().any(|(k, _)| k == "className") {
                    Some(*id)
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("flip button");
    let patches = rt.dispatch(flip, "onClick").expect("unmount");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.contains(&"will-unmount".to_string()),
        "willUnmount at unmount: {logs:?}"
    );
}

#[test]
fn class_composes_with_function_component() {
    let src = r#"
        class Classy extends Component {
            state = "cl";
            render() { return <p className="c">{this.state}</p>; }
        }
        component Fn() {
            return <span className="f">fn</span>;
        }
        component App() {
            return <div><Fn/><Classy/></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("<span") && tree.contains("<p") && tree.contains("cl"),
        "both render: {tree}"
    );
}

#[test]
fn class_render_with_member_expression_props() {
    // <Classy label={this.state}/> style usage via a class method reading
    // props is not in scope; here the class itself can render a component.
    let src = r#"
        component Header(t) {
            return <h1 className="h">{t}</h1>;
        }
        class Page extends Component {
            state = "title";
            render() {
                return <div><Header t={this.state}/><p className="p">body</p></div>;
            }
        }
        export default Page;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("title"),
        "class render passes state to children: {tree}"
    );
}
