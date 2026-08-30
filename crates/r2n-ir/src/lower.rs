//! Lowering: `r2n_ast::Program` -> `RuntimeTemplate`.
//!
//! This pass implements the architecture's lowering pipeline:
//!   ast -> js-ir -> react-ir -> runtime-ir
//! Specifically:
//!   * AST expressions become `JsExpr` (the JS IR), performing ANF-style
//!     capture of `let`/`const` bindings so the body is a closed tree.
//!   * JSX elements become `ReactNode` (the React IR), referencing component
//!     ids — the ADR-002 interlink.
//!   * Each component becomes a `RuntimeComponent` (the runtime IR) with its
//!     bindings and a `ReactNode` body — the template/instance split (ADR-010).

use crate::js::{JsBinOp, JsExpr, JsUnOp};
use crate::react::{ComponentRef, ReactNode};
use crate::runtime::{RuntimeComponent, RuntimeTemplate};
use r2n_ast::expr::{Element, Expr, Prop};
use r2n_ast::op::{BinOp, UnOp};
use r2n_ast::program::{Component, Decl, Program, Stmt};
use std::collections::HashMap;

/// Error during lowering (e.g. unknown component reference).
#[derive(Debug, Clone, PartialEq)]
pub enum LowerError {
    /// A component referenced by `<Name/>` or a call was never defined.
    UnknownComponent(String),
    /// A `return` produced a non-renderable expression.
    NonRenderableReturn(String),
    /// A `list.map(...)` with the wrong shape was used in child position.
    InvalidListMap(String),
    /// A `<>` fragment was given an attribute other than `key` (React
    /// fragments accept only `key`; other attributes would be silently
    /// meaningless).
    InvalidFragmentProp(String),
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::UnknownComponent(n) => write!(f, "unknown component '{n}'"),
            LowerError::NonRenderableReturn(s) => write!(f, "non-renderable return: {s}"),
            LowerError::InvalidListMap(s) => write!(f, "invalid list .map(): {s}"),
            LowerError::InvalidFragmentProp(n) => {
                write!(f, "fragment `<>` accepts only `key`, got `{n}`")
            }
        }
    }
}

impl std::error::Error for LowerError {}

/// Lower a whole program into a runtime template.
pub fn lower(program: &Program) -> Result<RuntimeTemplate, LowerError> {
    // 1. Build the component name -> index table.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut components: Vec<RuntimeComponent> = Vec::new();
    for decl in &program.decls {
        if let Decl::Component(c) = decl {
            index.insert(c.name.clone(), components.len());
            components.push(RuntimeComponent {
                name: c.name.clone(),
                params: c.params.clone(),
                captures: Vec::new(),
                bindings: Vec::new(),
                body: ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
            });
        }
    }
    let root = program
        .root
        .as_ref()
        .and_then(|r| index.get(r).copied())
        .ok_or_else(|| LowerError::UnknownComponent(program.root.clone().unwrap_or_default()))?;

    // 2. Lower each component body.
    for decl in &program.decls {
        if let Decl::Component(c) = decl {
            let idx = index[&c.name];
            let lowered = lower_component(c, &index)?;
            components[idx] = lowered;
        }
    }

    Ok(RuntimeTemplate {
        components,
        root,
        manifest: RuntimeTemplate::new().manifest,
    })
}

/// A component body being lowered: an ordered list of render-time steps.
/// `let`/`const` bindings and side-effecting expression statements
/// (`useEffect(...)`, `console.log(...)`) appear in SOURCE order, exactly like
/// React runs the function body top-to-bottom; the final element set ends
/// with the returned tree's prerequisites.
struct LoweredBody {
    /// Ordered (name, expr) steps; `$stmt` marks a pure side-effect step whose
    /// value is discarded but whose evaluation order matters.
    bindings: Vec<(String, JsExpr)>,
    body: ReactNode,
}

fn lower_component(
    c: &Component,
    index: &HashMap<String, usize>,
) -> Result<RuntimeComponent, LowerError> {
    let mut out = LoweredBody {
        bindings: Vec::new(),
        body: ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
    };
    for stmt in &c.body {
        match stmt {
            Stmt::Let { name, value } | Stmt::Const { name, value } => {
                out.bindings.push((name.clone(), lower_expr(value, index)?));
            }
            // A bare expression statement runs at render time, in source
            // order (this is how `useEffect(fn, deps)` registers).
            Stmt::Expr(e) => out
                .bindings
                .push(("$stmt".to_string(), lower_expr(e, index)?)),
            Stmt::Return(expr) => {
                out.body = lower_renderable(expr, index)?;
            }
        }
    }
    if !matches!(
        out.body,
        ReactNode::Host { .. }
            | ReactNode::Component { .. }
            | ReactNode::If { .. }
            | ReactNode::List { .. }
    ) {
        return Err(LowerError::NonRenderableReturn(format!(
            "component {} returns a non-renderable expression",
            c.name
        )));
    }
    // Captures = free variables of the whole lowered body that are not params
    // and not locally bound. The runtime's frame protocol supplies them.
    // `$stmt` steps define nothing, so any name they reference that appears in
    // a LATER binding is still free here — which is correct: React would also
    // fail to see a `let` that hasn't run yet. Source order makes the common
    // case (declare, then use) work naturally.
    let local_names: std::collections::HashSet<String> = c
        .params
        .iter()
        .chain(out.bindings.iter().map(|(n, _)| n))
        .cloned()
        .collect();
    let captures = free_vars_of_body(&out.bindings, &out.body)
        .into_iter()
        .filter(|n| !local_names.contains(n))
        .collect();
    Ok(RuntimeComponent {
        name: c.name.clone(),
        params: c.params.clone(),
        captures,
        bindings: out.bindings,
        body: out.body,
    })
}

/// Collect the free variables of a whole component body (bindings + return
/// node), treating names bound by params and preceding `let`s as bound.
fn free_vars_of_body(bindings: &[(String, JsExpr)], body: &ReactNode) -> Vec<String> {
    let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut free: Vec<String> = Vec::new();
    for (n, e) in bindings {
        collect_free(e, &bound, &mut free);
        bound.insert(n.clone());
    }
    collect_free_node(body, &bound, &mut free);
    free.sort();
    free.dedup();
    free
}

/// Lower an arbitrary expression into JS IR. `index` maps component names to
/// table indices; it is used by the renderable/lower_element paths and kept in
/// this uniform signature for consistency across the lowering functions.
#[allow(clippy::only_used_in_recursion)]
fn lower_expr(expr: &Expr, index: &HashMap<String, usize>) -> Result<JsExpr, LowerError> {
    Ok(match expr {
        Expr::Literal(l) => JsExpr::Lit(l.clone()),
        Expr::Ident { name, .. } => JsExpr::Var(name.clone()),
        Expr::Member { base, prop } => JsExpr::Get {
            base: Box::new(lower_expr(base, index)?),
            prop: prop.clone(),
        },
        Expr::Binary { op, left, right } => JsExpr::Bin {
            op: lower_binop(*op),
            left: Box::new(lower_expr(left, index)?),
            right: Box::new(lower_expr(right, index)?),
        },
        Expr::Unary { op, expr } => JsExpr::Un {
            op: lower_unop(*op),
            expr: Box::new(lower_expr(expr, index)?),
        },
        Expr::Call { callee, args } => {
            // Index access `arr[idx]` is emitted by the parser as
            // `Call(Member(base, "get"), [idx])`; lower it to `JsExpr::Index`.
            if let Expr::Member { base, prop } = &**callee {
                if prop == "get" && args.len() == 1 {
                    return Ok(JsExpr::Index {
                        base: Box::new(lower_expr(base, index)?),
                        key: Box::new(lower_expr(&args[0], index)?),
                    });
                }
            }
            JsExpr::Call {
                callee: Box::new(lower_expr(callee, index)?),
                args: args
                    .iter()
                    .map(|a| lower_expr(a, index))
                    .collect::<Result<_, _>>()?,
            }
        }
        Expr::Array(items) => JsExpr::Array(
            items
                .iter()
                .map(|i| lower_expr(i, index))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Ternary { cond, then, else_ } => JsExpr::If {
            cond: Box::new(lower_expr(cond, index)?),
            then: Box::new(lower_expr(then, index)?),
            else_: Box::new(lower_expr(else_, index)?),
        },
        Expr::Arrow { params, body } => JsExpr::Closure {
            params: params.clone(),
            captures: vec![], // captures computed lazily by the runtime frame
            body: Box::new(lower_expr(body, index)?),
        },
        Expr::Block(stmts) => JsExpr::Block(
            stmts
                .iter()
                .map(|s| lower_expr(s, index))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Assign { target, value } => JsExpr::Assign {
            target: Box::new(lower_expr(target, index)?),
            value: Box::new(lower_expr(value, index)?),
        },
        Expr::Element(_) => {
            return Err(LowerError::InvalidListMap(
                "element used as a value".to_string(),
            ))
        }
    })
}

fn lower_binop(op: BinOp) -> JsBinOp {
    match op {
        BinOp::Add => JsBinOp::Add,
        BinOp::Sub => JsBinOp::Sub,
        BinOp::Mul => JsBinOp::Mul,
        BinOp::Div => JsBinOp::Div,
        BinOp::Mod => JsBinOp::Mod,
        BinOp::Eq => JsBinOp::Eq,
        BinOp::Neq => JsBinOp::Neq,
        BinOp::Lt => JsBinOp::Lt,
        BinOp::Gt => JsBinOp::Gt,
        BinOp::Le => JsBinOp::Le,
        BinOp::Ge => JsBinOp::Ge,
        BinOp::And => JsBinOp::And,
        BinOp::Or => JsBinOp::Or,
    }
}

fn lower_unop(op: UnOp) -> JsUnOp {
    match op {
        UnOp::Neg => JsUnOp::Neg,
        UnOp::Not => JsUnOp::Not,
    }
}

/// Lower an expression that appears in a *render* position into a React node.
/// This is where JSX becomes the React IR and components become `ComponentRef`s.
fn lower_renderable(expr: &Expr, index: &HashMap<String, usize>) -> Result<ReactNode, LowerError> {
    match expr {
        Expr::Element(e) => lower_element(e, index),
        Expr::Ternary { cond, then, else_ } => Ok(ReactNode::If {
            cond: lower_expr(cond, index)?,
            then: Box::new(lower_renderable(then, index)?),
            else_: Box::new(lower_renderable(else_, index)?),
        }),
        // The `children` prop in render position: splice point for the
        // parent's passed-in children (React children composition).
        Expr::Ident {
            name,
            is_component: false,
        } if name == "children" => Ok(ReactNode::Children),
        // A bare `{expr}` as the sole child renders as text.
        other => Ok(ReactNode::Text(lower_expr(other, index)?)),
    }
}

fn lower_element(e: &Element, index: &HashMap<String, usize>) -> Result<ReactNode, LowerError> {
    // Fragment shorthand `<>...</>` (parsed as an Element with an empty tag):
    // a group of children with no host element of its own. React fragments
    // accept only a `key`; any other attribute is an error, not a silent drop.
    if e.tag.is_empty() {
        let mut key = None;
        for p in &e.props {
            if p.name == "key" {
                key = Some(match &p.value {
                    Some(v) => lower_expr(v, index)?,
                    None => JsExpr::Lit(r2n_ast::lit::Literal::Bool(true)),
                });
            } else {
                return Err(LowerError::InvalidFragmentProp(p.name.clone()));
            }
        }
        let mut children = Vec::with_capacity(e.children.len());
        for child in &e.children {
            children.push(lower_child(child, index)?);
        }
        return Ok(ReactNode::Fragment { key, children });
    }

    // Component element? (uppercase tag)
    if e.is_component {
        let comp_idx = *index
            .get(&e.tag)
            .ok_or_else(|| LowerError::UnknownComponent(e.tag.clone()))?;
        let mut props = lower_props(&e.props, index)?;
        // React children composition: JSX children of a component element
        // become its `children` prop. They are lowered NOW, in the parent's
        // context, and carried as pre-built React-IR nodes (a `Value::Children`
        // at runtime) — the child splices them via `ReactNode::Children`.
        if !e.children.is_empty() {
            let mut children = Vec::with_capacity(e.children.len());
            for child in &e.children {
                children.push(lower_child(child, index)?);
            }
            props.push(("children".to_string(), JsExpr::Children(children)));
        }
        return Ok(ReactNode::Component {
            component: ComponentRef(comp_idx),
            props,
        });
    }

    // Host element. Detect the special `children.map(...)` list form: when the
    // sole child is a `.map` call whose arrow body is a JSX element, we model it
    // as a keyed `ReactNode::List` nested inside the host's children (the
    // runtime flattens it into the host's child list at render time).
    if e.children.len() == 1 {
        if let Some(list) = try_lower_list(&e.children[0], index)? {
            let props = lower_props(&e.props, index)?;
            return Ok(ReactNode::Host {
                tag: e.tag.clone(),
                props,
                children: vec![list],
            });
        }
    }

    let props = lower_props(&e.props, index)?;
    let mut children = Vec::with_capacity(e.children.len());
    for child in &e.children {
        children.push(lower_child(child, index)?);
    }
    Ok(ReactNode::Host {
        tag: e.tag.clone(),
        props,
        children,
    })
}

/// Lower a child expression node (text/element/conditional/list).
fn lower_child(child: &Expr, index: &HashMap<String, usize>) -> Result<ReactNode, LowerError> {
    match child {
        Expr::Element(_) => lower_renderable(child, index),
        Expr::Ternary { .. } => lower_renderable(child, index),
        // `children` as a host element's child: the splice point.
        Expr::Ident {
            name,
            is_component: false,
        } if name == "children" => Ok(ReactNode::Children),
        Expr::Call { callee, .. } => {
            // possible `items.map(x => <li/>)` directly as a child
            if let Some(list) = try_lower_list(child, index)? {
                Ok(list)
            } else if is_index_call(callee) {
                // `arr[i]` (the parser emits `.get(i)`): renders the indexed
                // value as text — not a list map.
                Ok(ReactNode::Text(lower_expr(child, index)?))
            } else {
                Err(LowerError::InvalidListMap(format!("{callee:?} as child")))
            }
        }
        // `{cond && <el/>}` / `{cond || <el/>}`: short-circuit rendering.
        Expr::Binary { .. } if is_short_circuit_render(child) => lower_short_circuit(child, index),
        // A `{expr}` child renders as text.
        other => Ok(ReactNode::Text(lower_expr(other, index)?)),
    }
}

/// Is this callee an index access `base[key]` (the parser emits it as
/// `base.get(key)`)? Index access in child position renders the value,
/// whereas other method calls are not renderable.
fn is_index_call(callee: &Expr) -> bool {
    matches!(callee, Expr::Member { prop, .. } if prop == "get")
}

/// Does this expression structurally render a node on one side of a
/// short-circuit (`&&`/`||`)? Elements and ternaries do; plain values
/// render as (nullish-suppressed) text through the Text path instead.
fn is_short_circuit_render(expr: &Expr) -> bool {
    match expr {
        Expr::Binary {
            op: BinOp::And | BinOp::Or,
            left,
            right,
        } => is_renderable_expr(left) || is_renderable_expr(right),
        _ => false,
    }
}

fn is_renderable_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Element(_) | Expr::Ternary { .. })
}

/// Lower `{cond && <el/>}` to `If { cond, then: el, else: nothing }` — and
/// `{cond || <el/>}` to `If { cond, then: nothing, else: el }`. "Nothing"
/// is an empty fragment: React renders falsy short-circuit results as
/// nothing at all.
fn lower_short_circuit(
    expr: &Expr,
    index: &HashMap<String, usize>,
) -> Result<ReactNode, LowerError> {
    let (op, left, right) = match expr {
        Expr::Binary { op, left, right } if matches!(op, BinOp::And | BinOp::Or) => {
            (op, left, right)
        }
        other => return Ok(ReactNode::Text(lower_expr(other, index)?)),
    };
    let nothing = || {
        Box::new(ReactNode::Fragment {
            key: None,
            children: Vec::new(),
        })
    };
    // The ELEMENT side is the renderable branch; the other side is the
    // condition (`cond && el` ⇒ el when cond; `cond || el` ⇒ el when !cond).
    // When both sides are renderable the left wins the branch, right the
    // condition — an unusual shape, but well-defined.
    let (cond_expr, node_side) = if is_renderable_expr(left) && !is_renderable_expr(right) {
        (right, left)
    } else {
        (left, right)
    };
    let node = lower_renderable(node_side, index)?;
    Ok(match op {
        BinOp::And => ReactNode::If {
            cond: lower_expr(cond_expr, index)?,
            then: Box::new(node),
            else_: nothing(),
        },
        BinOp::Or => ReactNode::If {
            cond: lower_expr(cond_expr, index)?,
            then: nothing(),
            else_: Box::new(node),
        },
        _ => unreachable!("guarded by is_short_circuit_render"),
    })
}

/// Detect and lower the `array.map((x, i) => <element key={x}/>)` pattern into
/// a `ReactNode::List`. Returns `Ok(None)` when the expr is not this pattern.
fn try_lower_list(
    expr: &Expr,
    index: &HashMap<String, usize>,
) -> Result<Option<ReactNode>, LowerError> {
    // Pattern: Call { callee: Member { base, prop: "map" }, args: [arrow] }
    let (base, arrow) = match expr {
        Expr::Call { callee, args } if args.len() == 1 => match &**callee {
            Expr::Member { base, prop } if prop == "map" => (base, &args[0]),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let (params, arrow_body) = match arrow {
        Expr::Arrow { params, body } => (params, body),
        _ => return Ok(None),
    };
    // The arrow body must be a JSX element (so each item becomes a node).
    let item = match &**arrow_body {
        Expr::Element(_) => lower_renderable(arrow_body, index)?,
        _ => return Ok(None),
    };
    // The per-element variable name is the arrow's first parameter.
    let item_var = params
        .first()
        .cloned()
        .unwrap_or_else(|| "$item".to_string());
    // Rewrite every occurrence of `item_var` in the item tree and key to the
    // runtime's reserved name `$item`, so the runtime can substitute the actual
    // element value at render time.
    let item = subst_node(item, &item_var, "$item");
    // The key expression: prefer the `key` prop of the item element; otherwise
    // the item value itself.
    let key_expr = match &item {
        ReactNode::Host { props, .. } | ReactNode::Component { props, .. } => {
            if let Some((_, k)) = props.iter().find(|(n, _)| n == "key") {
                subst_expr(k.clone(), &item_var, "$item")
            } else {
                // Default key = the item value itself.
                JsExpr::Var("$item".to_string())
            }
        }
        // A fragment item carries its key in the Fragment node (the only
        // prop fragments accept). Substitution already rewrote item vars.
        ReactNode::Fragment { key: Some(k), .. } => k.clone(),
        _ => JsExpr::Var("$item".to_string()),
    };
    Ok(Some(ReactNode::List {
        items: lower_expr(base, index)?,
        key_expr,
        item: Box::new(item),
    }))
}

/// Substitute `from` -> `to` inside a JS expression tree.
fn subst_expr(e: JsExpr, from: &str, to: &str) -> JsExpr {
    match e {
        JsExpr::Var(v) if v == from => JsExpr::Var(to.to_string()),
        JsExpr::Var(v) => JsExpr::Var(v),
        JsExpr::Lit(l) => JsExpr::Lit(l),
        JsExpr::Get { base, prop } => JsExpr::Get {
            base: Box::new(subst_expr(*base, from, to)),
            prop,
        },
        JsExpr::Index { base, key } => JsExpr::Index {
            base: Box::new(subst_expr(*base, from, to)),
            key: Box::new(subst_expr(*key, from, to)),
        },
        JsExpr::Bin { op, left, right } => JsExpr::Bin {
            op,
            left: Box::new(subst_expr(*left, from, to)),
            right: Box::new(subst_expr(*right, from, to)),
        },
        JsExpr::Un { op, expr } => JsExpr::Un {
            op,
            expr: Box::new(subst_expr(*expr, from, to)),
        },
        JsExpr::Call { callee, args } => JsExpr::Call {
            callee: Box::new(subst_expr(*callee, from, to)),
            args: args.into_iter().map(|a| subst_expr(a, from, to)).collect(),
        },
        JsExpr::Closure {
            params,
            captures,
            body,
        } => JsExpr::Closure {
            params,
            captures,
            body: Box::new(subst_expr(*body, from, to)),
        },
        JsExpr::Array(items) => {
            JsExpr::Array(items.into_iter().map(|i| subst_expr(i, from, to)).collect())
        }
        JsExpr::Block(stmts) => {
            JsExpr::Block(stmts.into_iter().map(|s| subst_expr(s, from, to)).collect())
        }
        JsExpr::Assign { target, value } => JsExpr::Assign {
            target: Box::new(subst_expr(*target, from, to)),
            value: Box::new(subst_expr(*value, from, to)),
        },
        JsExpr::If { cond, then, else_ } => JsExpr::If {
            cond: Box::new(subst_expr(*cond, from, to)),
            then: Box::new(subst_expr(*then, from, to)),
            else_: Box::new(subst_expr(*else_, from, to)),
        },
        JsExpr::Builtin(b) => JsExpr::Builtin(b),
        JsExpr::Children(nodes) => {
            JsExpr::Children(nodes.into_iter().map(|n| subst_node(n, from, to)).collect())
        }
    }
}

/// Substitute `from` -> `to` inside a React node tree (props + children).
fn subst_node(n: ReactNode, from: &str, to: &str) -> ReactNode {
    match n {
        ReactNode::Host {
            tag,
            props,
            children,
        } => ReactNode::Host {
            tag,
            props: props
                .into_iter()
                .map(|(k, v)| (k, subst_expr(v, from, to)))
                .collect(),
            children: children
                .into_iter()
                .map(|c| subst_node(c, from, to))
                .collect(),
        },
        ReactNode::Component { component, props } => ReactNode::Component {
            component,
            props: props
                .into_iter()
                .map(|(k, v)| (k, subst_expr(v, from, to)))
                .collect(),
        },
        ReactNode::If { cond, then, else_ } => ReactNode::If {
            cond: subst_expr(cond, from, to),
            then: Box::new(subst_node(*then, from, to)),
            else_: Box::new(subst_node(*else_, from, to)),
        },
        ReactNode::List {
            items,
            key_expr,
            item,
        } => ReactNode::List {
            items: subst_expr(items, from, to),
            key_expr: subst_expr(key_expr, from, to),
            item: Box::new(subst_node(*item, from, to)),
        },
        ReactNode::Text(e) => ReactNode::Text(subst_expr(e, from, to)),
        ReactNode::Children => ReactNode::Children,
        ReactNode::Fragment { key, children } => ReactNode::Fragment {
            key: key.map(|k| subst_expr(k, from, to)),
            children: children
                .into_iter()
                .map(|c| subst_node(c, from, to))
                .collect(),
        },
    }
}

/// Collect free variables of a JS IR expression: names referenced but not
/// bound by `bound` (params + preceding `let`s) or shadowed by closure params.
fn collect_free(e: &JsExpr, bound: &std::collections::HashSet<String>, out: &mut Vec<String>) {
    match e {
        JsExpr::Var(v) => {
            if !bound.contains(v) && !out.contains(v) {
                out.push(v.clone());
            }
        }
        JsExpr::Lit(_) | JsExpr::Builtin(_) => {}
        JsExpr::Children(nodes) => {
            for n in nodes {
                collect_free_node(n, bound, out);
            }
        }
        JsExpr::Get { base, .. } => collect_free(base, bound, out),
        JsExpr::Index { base, key } => {
            collect_free(base, bound, out);
            collect_free(key, bound, out);
        }
        JsExpr::Bin { left, right, .. } => {
            collect_free(left, bound, out);
            collect_free(right, bound, out);
        }
        JsExpr::Un { expr, .. } => collect_free(expr, bound, out),
        JsExpr::Call { callee, args } => {
            collect_free(callee, bound, out);
            for a in args {
                collect_free(a, bound, out);
            }
        }
        JsExpr::Closure { params, body, .. } => {
            let mut inner = bound.clone();
            for p in params {
                inner.insert(p.clone());
            }
            collect_free(body, &inner, out);
        }
        JsExpr::Array(items) => {
            for i in items {
                collect_free(i, bound, out);
            }
        }
        JsExpr::Block(stmts) => {
            for s in stmts {
                collect_free(s, bound, out);
            }
        }
        JsExpr::Assign { target, value } => {
            collect_free(target, bound, out);
            collect_free(value, bound, out);
        }
        JsExpr::If { cond, then, else_ } => {
            collect_free(cond, bound, out);
            collect_free(then, bound, out);
            collect_free(else_, bound, out);
        }
    }
}

/// Collect free variables of a React node tree (props + children + keys).
fn collect_free_node(
    n: &ReactNode,
    bound: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    match n {
        ReactNode::Host {
            props, children, ..
        } => {
            for (_, e) in props {
                collect_free(e, bound, out);
            }
            for c in children {
                collect_free_node(c, bound, out);
            }
        }
        ReactNode::Component { props, .. } => {
            for (_, e) in props {
                collect_free(e, bound, out);
            }
        }
        ReactNode::If { cond, then, else_ } => {
            collect_free(cond, bound, out);
            collect_free_node(then, bound, out);
            collect_free_node(else_, bound, out);
        }
        ReactNode::List {
            items,
            key_expr,
            item,
        } => {
            collect_free(items, bound, out);
            collect_free(key_expr, bound, out);
            collect_free_node(item, bound, out);
        }
        ReactNode::Text(e) => collect_free(e, bound, out),
        // The children splice point is a runtime concern; `children` itself
        // is a param/prop the component received (already counted if bound).
        ReactNode::Children => {}
        ReactNode::Fragment { key, children } => {
            if let Some(k) = key {
                collect_free(k, bound, out);
            }
            for c in children {
                collect_free_node(c, bound, out);
            }
        }
    }
}

fn lower_props(
    props: &[Prop],
    index: &HashMap<String, usize>,
) -> Result<Vec<(String, JsExpr)>, LowerError> {
    let mut out = Vec::with_capacity(props.len());
    for p in props {
        let value = match &p.value {
            Some(v) => lower_expr(v, index)?,
            None => JsExpr::Lit(r2n_ast::lit::Literal::Bool(true)),
        };
        out.push((p.name.clone(), value));
    }
    Ok(out)
}
