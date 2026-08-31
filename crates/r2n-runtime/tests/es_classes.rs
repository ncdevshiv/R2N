//! M2-T04 acceptance: classes, `this`, `new`.
//!
//! ECMAScript semantics under test:
//! 1. `class P extends Object { constructor(...) {...} }` — `new P(args)`
//!    allocates an instance, runs the constructor with `this`, returns it.
//! 2. Instance fields set via `this.x = v` are read back (own props).
//! 3. Prototype methods (`method() {...}`) are shared and called with
//!    `this` = receiver: `p.sum()` reads `p.x + p.y`.
//! 4. Methods are on the prototype chain (getPrototypeOf(p) exposes them
//!    to the instance).
//! 5. Inheritance: `class C extends B` — is NOT yet implemented (React
//!    class components' `extends Component` is the only existing base);
//!    T04 acceptance covers the constructor/method/this core; inheritance
//!    via ES `extends` is a documented follow-on (T04 core).
//! 6. Two instances are independent; two `new`s yield distinct objects.

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

fn texts(r: &MemoryRenderer) -> Vec<String> {
    r.nodes()
        .values()
        .filter_map(|n| match n {
            r2n_renderer_memory::MemNode::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn new_allocates_and_runs_constructor() {
    let src = r#"
        class Point extends Object {
            constructor(x, y) {
                this.x = x;
                this.y = y;
            }
            sum() {
                return this.x + this.y;
            }
        }
        component App() {
            let p = new Point(3, 4);
            return <div><p className="x">{p.x}</p><p className="y">{p.y}</p><p className="sum">{p.sum()}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "3"), "x: {t:?}");
    assert!(t.iter().any(|x| x == "4"), "y: {t:?}");
    assert!(
        t.iter().any(|x| x == "7"),
        "sum via prototype method: {t:?}"
    );
}

#[test]
fn method_on_prototype_chain() {
    let src = r#"
        class Person extends Object {
            constructor(name) {
                this.name = name;
            }
            greet() {
                return "hi " + this.name;
            }
        }
        component App() {
            let p = new Person("ana");
            let proto = Object.getPrototypeOf(p);
            let viaProto = proto.greet();
            let viaInst = p.greet();
            return <div><p className="v">{viaInst}</p><p className="p">{viaProto}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(
        t.iter().any(|x| x == "hi ana"),
        "method on instance via prototype: {t:?}"
    );
}

#[test]
fn instances_are_independent() {
    let src = r#"
        class Counter extends Object {
            constructor() {
                this.n = 0;
            }
            inc() {
                this.n = this.n + 1;
                return this.n;
            }
        }
        component App() {
            let a = new Counter();
            let b = new Counter();
            let a1 = a.inc();
            let a2 = a.inc();
            let b1 = b.inc();
            return <div><p className="a1">{a1}</p><p className="a2">{a2}</p><p className="b1">{b1}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "1"), "a.inc 1: {t:?}");
    assert!(t.iter().any(|x| x == "2"), "a.inc 2: {t:?}");
    assert!(t.iter().any(|x| x == "1"), "b.inc independent: {t:?}");
}

#[test]
fn this_in_constructor_is_not_leaked() {
    let src = r#"
        class P extends Object {
            constructor() {
                this.only = "ctor";
            }
        }
        component App() {
            let p = new P();
            return <div><p className="o">{p.only}</p><p className="t">{typeof(this)}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "ctor"), "ctor field: {t:?}");
    assert!(
        t.iter().any(|x| x == "undefined"),
        "no global this leak (typeof(this)=undefined outside): {t:?}"
    );
}

#[test]
fn methods_can_mutate_instance_across_calls() {
    let src = r#"
        class Bank extends Object {
            constructor() {
                this.balance = 100;
            }
            deposit(amt) {
                this.balance = this.balance + amt;
                return this.balance;
            }
        }
        component App() {
            let b = new Bank();
            let s1 = b.deposit(50);
            let s2 = b.deposit(25);
            return <div><p className="s1">{s1}</p><p className="s2">{s2}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "150"), "after 50: {t:?}");
    assert!(t.iter().any(|x| x == "175"), "after 25: {t:?}");
}
