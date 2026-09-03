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
use crate::runtime::{ClassInfo, ClassMethod, RuntimeComponent, RuntimeTemplate};
use r2n_ast::expr::{Element, Expr, Prop};
use r2n_ast::op::{BinOp, UnOp};
use r2n_ast::program::{ClassComponent, Component, Decl, Param, Pattern, Program, Stmt};
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
    /// `await` used outside its supported positions (M2-T07: `await` is a
    /// statement value inside an async body — anything else is a precise
    /// compile error, not a silent miscompile).
    UnsupportedAwait(String),
    /// A `<>` fragment was given an attribute other than `key` (React
    /// fragments accept only `key`; other attributes would be silently
    /// meaningless).
    InvalidFragmentProp(String),
    /// A statement or expression form the lowerer does not support yet
    /// (general statement grammar, T09b/T10): precise compile error naming
    /// the construct and its source position context.
    Unsupported(String),
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
            LowerError::UnsupportedAwait(s) => write!(f, "unsupported await: {s}"),
            LowerError::Unsupported(s) => write!(f, "unsupported construct: {s}"),
        }
    }
}

impl std::error::Error for LowerError {}

/// Lower a whole program into a runtime template. Production: StrictMode
/// nodes are STRIPPED (dev semantics must not reach production artifacts).
pub fn lower(program: &Program) -> Result<RuntimeTemplate, LowerError> {
    lower_with(program, false)
}

/// Dev-mode lowering: keeps StrictMode nodes and marks the template.
pub fn lower_dev(program: &Program) -> Result<RuntimeTemplate, LowerError> {
    lower_with(program, true)
}

/// Module-aware lowering (M2-T09): lower a single module's declarations into
/// runtime IR, resolving every component reference (own declarations AND
/// imported bindings) through `names` — a map of referenceable name -> GLOBAL
/// component index. The linker builds `names` per module so cross-module
/// imports (static `import`, and the `export default` / `export {}` surfaces)
/// resolve to the target's global index.
///
/// Returns the module's components keyed by their global index (the linker
/// pre-assigns contiguous indices per module, so there are no gaps), its
/// generator functions, and the `export default` name if present. StrictMode
/// stripping is deliberately NOT performed here: the linker strips the merged
/// artifact once, from the entry build's mode.
pub type LoweredModuleParts = (
    Vec<(usize, RuntimeComponent)>,
    Vec<crate::runtime::GeneratorIr>,
    Vec<crate::runtime::FuncIr>,
    Vec<(String, JsExpr)>,
    Option<String>,
);

pub fn lower_module_parts(
    program: &Program,
    names: &HashMap<String, usize>,
) -> Result<LoweredModuleParts, LowerError> {
    let mut parts = Vec::new();
    let mut generators = Vec::new();
    let mut functions = Vec::new();
    let mut top_levels = Vec::new();
    let mut default = None;
    for decl in &program.decls {
        match decl {
            Decl::Component(c) => {
                let idx = names
                    .get(&c.name)
                    .copied()
                    .ok_or_else(|| LowerError::UnknownComponent(c.name.clone()))?;
                parts.push((idx, lower_component(c, names)?));
            }
            Decl::Class(c) => {
                let idx = names
                    .get(&c.name)
                    .copied()
                    .ok_or_else(|| LowerError::UnknownComponent(c.name.clone()))?;
                parts.push((idx, lower_class(c, names)?));
            }
            Decl::GeneratorFn(g) => generators.push(lower_generator_fn(g, names)?),
            Decl::FuncDecl(f) => functions.push(lower_func_decl(f, names)?),
            Decl::TopLevel { pattern, value, .. } => {
                top_levels.extend(lower_top_level(pattern, value, names)?);
            }
            Decl::ExportDefault(name) => default = Some(name.clone()),
            Decl::ExportDecl(e) => match e {
                r2n_ast::program::ExportDecl::Function(f) => {
                    // Exported functions are COMPONENTS (React semantics —
                    // the linker pre-assigned a table index): lower the
                    // params+body as a component. A non-JSX return is a
                    // precise NonRenderableReturn, not a silent miscompile.
                    let idx = names
                        .get(&f.name)
                        .copied()
                        .ok_or_else(|| LowerError::UnknownComponent(f.name.clone()))?;
                    parts.push((
                        idx,
                        lower_component(
                            &Component {
                                name: f.name.clone(),
                                params: f.params.clone(),
                                body: f.body.clone(),
                            },
                            names,
                        )?,
                    ));
                }
                r2n_ast::program::ExportDecl::Const { name, value } => {
                    // `export const Name = memo(function...)` — a component
                    // through the memo HOF (index pre-assigned); any other
                    // const is a module value.
                    match component_fn_of(value) {
                        Some((params, body)) => {
                            let idx = names
                                .get(name)
                                .copied()
                                .ok_or_else(|| LowerError::UnknownComponent(name.clone()))?;
                            parts.push((
                                idx,
                                lower_component(
                                    &Component {
                                        name: name.clone(),
                                        params: params.clone(),
                                        body: body.clone(),
                                    },
                                    names,
                                )?,
                            ));
                        }
                        None => top_levels.push((name.clone(), lower_expr(value, names)?)),
                    }
                }
            },
            Decl::Import(_) | Decl::ExportNamed(_) => {}
        }
    }
    Ok((parts, generators, functions, top_levels, default))
}

/// Lower a plain `function name(params) { stmts }` to a `FuncIr`: the body
/// becomes a Block of Assigns (locals) ending in the terminal value, exactly
/// like an ES class method body — `return e` raises through the runtime's
/// control-flow channel and is caught at the call boundary.
fn lower_func_decl(
    f: &r2n_ast::program::FuncDecl,
    index: &HashMap<String, usize>,
) -> Result<crate::runtime::FuncIr, LowerError> {
    let (params, mut stmts) = lower_param_binds(&f.params, index, &format!("function {}", f.name))?;
    for st in &f.body {
        lower_stmt(st, &f.name, index, &mut stmts)?;
    }
    Ok(crate::runtime::FuncIr {
        name: f.name.clone(),
        params,
        body: JsExpr::Block(stmts),
    })
}

/// Lower one statement of a plain-function body into `out` (general statement
/// grammar: `if`/`while`/`for`/`switch`/`break`/`continue`/destructuring all
/// lower to their IR drivers; control flow travels the runtime's error
/// channel and is caught by the loop/switch drivers and the call boundary).
fn lower_stmt(
    st: &Stmt,
    fn_name: &str,
    index: &HashMap<String, usize>,
    out: &mut Vec<JsExpr>,
) -> Result<(), LowerError> {
    match st {
        Stmt::Let { name, value } | Stmt::Const { name, value } => {
            out.push(JsExpr::Assign {
                target: Box::new(JsExpr::Var(name.clone())),
                value: Box::new(lower_expr(value, index)?),
            });
        }
        Stmt::Destructure { pattern, value, .. } => {
            lower_destructure(pattern, &lower_expr(value, index)?, index, out)?;
        }
        Stmt::Return(expr) => {
            // `return e` raises; the call boundary catches it. (A bare value
            // without Return would fall through to the next statement —
            // wrong for early returns like the reducer's `case: return ...`.)
            out.push(JsExpr::Return(Some(Box::new(lower_expr(expr, index)?))));
        }
        Stmt::Expr(e) => out.push(lower_expr(e, index)?),
        Stmt::If { cond, then, else_ } => {
            let mut then_out = Vec::new();
            for s in then {
                lower_stmt(s, fn_name, index, &mut then_out)?;
            }
            let mut else_out = Vec::new();
            if let Some(ss) = else_ {
                for s in ss {
                    lower_stmt(s, fn_name, index, &mut else_out)?;
                }
            } else {
                else_out.push(JsExpr::Lit(r2n_ast::lit::Literal::Null));
            }
            out.push(JsExpr::If {
                cond: Box::new(lower_expr(cond, index)?),
                then: Box::new(JsExpr::Block(then_out)),
                else_: Box::new(JsExpr::Block(else_out)),
            });
        }
        Stmt::While { cond, body } => {
            let mut body_out = Vec::new();
            for s in body {
                lower_stmt(s, fn_name, index, &mut body_out)?;
            }
            out.push(JsExpr::While {
                cond: Box::new(lower_expr(cond, index)?),
                body: Box::new(JsExpr::Block(body_out)),
                step: None,
            });
        }
        Stmt::For {
            init,
            cond,
            update,
            body,
        } => {
            // `for (init; cond; update) body` -> `init; while (cond ??
            // true) { body } step update`: the step runs after every
            // iteration INCLUDING `continue`, but not after `break` (ECMA).
            if let Some(i) = init {
                lower_stmt(i, fn_name, index, out)?;
            }
            let mut body_out = Vec::new();
            for s in body {
                lower_stmt(s, fn_name, index, &mut body_out)?;
            }
            let cond_e = match cond {
                Some(c) => lower_expr(c, index)?,
                None => JsExpr::Lit(r2n_ast::lit::Literal::Bool(true)),
            };
            let step = match update {
                Some(u) => Some(Box::new(lower_expr(u, index)?)),
                None => None,
            };
            out.push(JsExpr::While {
                cond: Box::new(cond_e),
                body: Box::new(JsExpr::Block(body_out)),
                step,
            });
        }
        Stmt::Switch { disc, cases } => {
            let mut lowered = Vec::new();
            let mut default = None;
            for (test, body) in cases {
                let mut body_out = Vec::new();
                for s in body {
                    lower_stmt(s, fn_name, index, &mut body_out)?;
                }
                match test {
                    Some(t) => lowered.push(crate::js::SwitchCase {
                        test: lower_expr(t, index)?,
                        body: body_out,
                    }),
                    None => default = Some(body_out),
                }
            }
            out.push(JsExpr::Switch {
                disc: Box::new(lower_expr(disc, index)?),
                cases: lowered,
                default,
            });
        }
        Stmt::Break => out.push(JsExpr::Break),
        Stmt::Continue => out.push(JsExpr::Continue),
    }
    let _ = fn_name;
    Ok(())
}

/// Lower a destructuring binding `pattern = value` into plain `Assign`s in
/// `out`: the value evaluates ONCE into a `$dst` temp, then each bound name
/// assigns from an index/get of the temp (`o[N]` for arrays, `.key` for
/// objects). Object/array `...rest` rebuilds the leftover as a fresh
/// array/object. `$dst`/`$dstN` are reserved (never user bindings).
fn lower_destructure(
    pattern: &Pattern,
    value: &JsExpr,
    index: &HashMap<String, usize>,
    out: &mut Vec<JsExpr>,
) -> Result<(), LowerError> {
    static DST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = DST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = format!("$dst{n}");
    out.push(JsExpr::Assign {
        target: Box::new(JsExpr::Var(tmp.clone())),
        value: Box::new(value.clone()),
    });
    lower_pattern_into(pattern, &JsExpr::Var(tmp), index, out)
}

/// Bind every name in `pattern` from the already-evaluated `src` expression.
fn lower_pattern_into(
    pattern: &Pattern,
    src: &JsExpr,
    index: &HashMap<String, usize>,
    out: &mut Vec<JsExpr>,
) -> Result<(), LowerError> {
    match pattern {
        Pattern::Name { name, default } => {
            let mut rhs = src.clone();
            if let Some(d) = default {
                // `x = dflt`: default applies when the value is `undefined`.
                rhs = JsExpr::If {
                    cond: Box::new(JsExpr::Bin {
                        op: JsBinOp::StrictEq,
                        left: Box::new(src.clone()),
                        right: Box::new(JsExpr::Lit(r2n_ast::lit::Literal::Undefined)),
                    }),
                    then: Box::new(lower_expr(d, index)?),
                    else_: Box::new(src.clone()),
                };
            }
            out.push(JsExpr::Assign {
                target: Box::new(JsExpr::Var(name.clone())),
                value: Box::new(rhs),
            });
        }
        Pattern::Object { props, rest } => {
            for pr in props {
                let field = JsExpr::Get {
                    base: Box::new(src.clone()),
                    prop: pr.key.clone(),
                };
                match &pr.alias {
                    Some(alias) => lower_pattern_into(alias, &field, index, out)?,
                    None => {
                        out.push(JsExpr::Assign {
                            target: Box::new(JsExpr::Var(pr.key.clone())),
                            value: Box::new(field),
                        });
                    }
                }
            }
            if let Some(r) = rest {
                // `...rest`: rebuild without the listed keys. The runtime
                // exposes this as a builtin member call on the source.
                out.push(JsExpr::Assign {
                    target: Box::new(JsExpr::Var(r.clone())),
                    value: Box::new(JsExpr::Call {
                        callee: Box::new(JsExpr::Get {
                            base: Box::new(src.clone()),
                            prop: "$rest".to_string(),
                        }),
                        args: props
                            .iter()
                            .map(|p| JsExpr::Lit(r2n_ast::lit::Literal::String(p.key.clone())))
                            .collect(),
                    }),
                });
            }
        }
        Pattern::Array { items, rest } => {
            for (i, it) in items.iter().enumerate() {
                if let Some(p) = it {
                    let elem = JsExpr::Index {
                        base: Box::new(src.clone()),
                        key: Box::new(JsExpr::Lit(r2n_ast::lit::Literal::Int(i as i64))),
                    };
                    lower_pattern_into(p, &elem, index, out)?;
                }
            }
            if let Some(r) = rest {
                out.push(JsExpr::Assign {
                    target: Box::new(JsExpr::Var(r.clone())),
                    value: Box::new(JsExpr::Call {
                        callee: Box::new(JsExpr::Get {
                            base: Box::new(src.clone()),
                            prop: "$restFrom".to_string(),
                        }),
                        args: vec![JsExpr::Lit(r2n_ast::lit::Literal::Int(items.len() as i64))],
                    }),
                });
            }
        }
    }
    Ok(())
}
/// Lower a top-level `let pattern = value`: plain names lower directly;
/// destructuring patterns expand to a `$tlN` temp plus one entry per bound
/// name (module init evaluates entries in order into the global env, so the
/// temp protocol works unchanged).
fn lower_top_level(
    pattern: &Pattern,
    value: &r2n_ast::expr::Expr,
    index: &HashMap<String, usize>,
) -> Result<Vec<(String, JsExpr)>, LowerError> {
    let v = lower_expr(value, index)?;
    match pattern {
        Pattern::Name { name, .. } => Ok(vec![(name.clone(), v)]),
        pattern => {
            let mut tmp = Vec::new();
            lower_destructure(pattern, &v, index, &mut tmp)?;
            let mut out = Vec::new();
            for e in tmp {
                match e {
                    JsExpr::Assign { target, value } => match *target {
                        JsExpr::Var(n) => out.push((n, *value)),
                        other => {
                            return Err(LowerError::Unsupported(format!(
                                "non-variable destructuring target in top-level let: {other:?}"
                            )));
                        }
                    },
                    other => {
                        return Err(LowerError::Unsupported(format!(
                            "non-assign in top-level destructuring expansion: {other:?}"
                        )));
                    }
                }
            }
            Ok(out)
        }
    }
}

fn lower_with(program: &Program, strict_mode: bool) -> Result<RuntimeTemplate, LowerError> {
    // 1. Build the component name -> index table.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut components: Vec<RuntimeComponent> = Vec::new();
    let mut generators: Vec<crate::runtime::GeneratorIr> = Vec::new();
    let mut functions: Vec<crate::runtime::FuncIr> = Vec::new();
    let mut top_levels: Vec<(String, JsExpr)> = Vec::new();
    for decl in &program.decls {
        // Exported functions and memo-wrapped consts are components (same
        // rule as the multi-module linker); everything else takes a slot by
        // its declared name.
        let name = match decl {
            Decl::Component(c) => c.name.clone(),
            Decl::Class(c) => c.name.clone(),
            Decl::ExportDecl(r2n_ast::program::ExportDecl::Function(f)) => f.name.clone(),
            Decl::ExportDecl(r2n_ast::program::ExportDecl::Const { name, value })
                if component_fn_of(value).is_some() =>
            {
                name.clone()
            }
            Decl::Import(_)
            | Decl::ExportDefault(_)
            | Decl::ExportNamed(_)
            | Decl::GeneratorFn(_)
            | Decl::FuncDecl(_)
            | Decl::ExportDecl(_)
            | Decl::TopLevel { .. } => continue,
        };
        index.insert(name.clone(), components.len());
        components.push(RuntimeComponent {
            name,
            params: Vec::new(),
            captures: Vec::new(),
            bindings: Vec::new(),
            body: ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
            class: None,
        });
    }
    let root = program
        .root
        .as_ref()
        .and_then(|r| index.get(r).copied())
        .ok_or_else(|| LowerError::UnknownComponent(program.root.clone().unwrap_or_default()))?;

    // 2. Lower each component body.
    for decl in &program.decls {
        match decl {
            Decl::Component(c) => {
                let idx = index[&c.name];
                let lowered = lower_component(c, &index)?;
                components[idx] = lowered;
            }
            Decl::Class(c) => {
                let idx = index[&c.name];
                components[idx] = lower_class(c, &index)?;
            }
            Decl::GeneratorFn(g) => {
                generators.push(lower_generator_fn(g, &index)?);
            }
            Decl::FuncDecl(f) => {
                functions.push(lower_func_decl(f, &index)?);
            }
            Decl::TopLevel { pattern, value, .. } => {
                top_levels.extend(lower_top_level(pattern, value, &index)?);
            }
            Decl::ExportDecl(e) => match e {
                r2n_ast::program::ExportDecl::Function(f) => {
                    let idx = index[&f.name];
                    components[idx] = lower_component(
                        &Component {
                            name: f.name.clone(),
                            params: f.params.clone(),
                            body: f.body.clone(),
                        },
                        &index,
                    )?;
                }
                r2n_ast::program::ExportDecl::Const { name, value } => {
                    match component_fn_of(value) {
                        Some((params, body)) => {
                            let idx = index[name];
                            components[idx] = lower_component(
                                &Component {
                                    name: name.clone(),
                                    params: params.clone(),
                                    body: body.clone(),
                                },
                                &index,
                            )?;
                        }
                        None => top_levels.push((name.clone(), lower_expr(value, &index)?)),
                    }
                }
            },
            _ => {}
        }
    }

    let mut template = RuntimeTemplate {
        components,
        root,
        generators,
        functions,
        top_levels,
        modules: Vec::new(),
        manifest: RuntimeTemplate::new().manifest,
        strict_mode: false,
    };
    if !strict_mode {
        // Production: the dev-only marker never crosses the artifact.
        for comp in &mut template.components {
            let b = std::mem::replace(
                &mut comp.body,
                ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
            );
            comp.body = strip_strict(b);
        }
    } else {
        // Dev: effects inside StrictMode subtrees double-invoke.
        template.strict_mode = true;
    }
    Ok(template)
}

/// Remove `<StrictMode>` wrappers (dev semantics out of production).
fn strip_strict(n: ReactNode) -> ReactNode {
    match n {
        ReactNode::StrictMode { children } => {
            // Transparent: children splice where the wrapper sat.
            ReactNode::Fragment {
                key: None,
                children: children.into_iter().map(strip_strict).collect(),
            }
        }
        ReactNode::Host {
            tag,
            props,
            children,
        } => ReactNode::Host {
            tag,
            props,
            children: children.into_iter().map(strip_strict).collect(),
        },
        ReactNode::Component { component, props } => ReactNode::Component { component, props },
        ReactNode::If { cond, then, else_ } => ReactNode::If {
            cond,
            then: Box::new(strip_strict(*then)),
            else_: Box::new(strip_strict(*else_)),
        },
        ReactNode::List {
            items,
            key_expr,
            item,
        } => ReactNode::List {
            items,
            key_expr,
            item: Box::new(strip_strict(*item)),
        },
        ReactNode::ContextProvider {
            ctx,
            value,
            children,
        } => ReactNode::ContextProvider {
            ctx,
            value,
            children: children.into_iter().map(strip_strict).collect(),
        },
        ReactNode::Portal { target, children } => ReactNode::Portal {
            target,
            children: children.into_iter().map(strip_strict).collect(),
        },
        ReactNode::Suspense { fallback, children } => ReactNode::Suspense {
            fallback: Box::new(strip_strict(*fallback)),
            children: children.into_iter().map(strip_strict).collect(),
        },
        other => other,
    }
}

/// Plain (non-pattern) parameter names from a `Vec<Param>`: the IR and
/// runtime still take explicit named params; destructuring params lower to
/// binds inside the body (full pattern support arrives with T10 lowering).
fn plain_param_names(params: &[Param]) -> Vec<String> {
    params
        .iter()
        .filter_map(|p| match &p.pattern {
            Pattern::Name { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Lower parameter bindings to `(positional_names, prepend_stmts)`: plain
/// names pass through; `x = dflt` prepends an `undefined`-guarded default
/// assign; destructuring patterns take a synthetic `$p{i}` positional and
/// prepend the pattern expansion. `...rest` is a precise error (call-site
/// arg-vector support has not landed).
fn lower_param_binds(
    params: &[Param],
    index: &HashMap<String, usize>,
    what: &str,
) -> Result<(Vec<String>, Vec<JsExpr>), LowerError> {
    let mut names = Vec::with_capacity(params.len());
    let mut prepend = Vec::new();
    for (i, p) in params.iter().enumerate() {
        if p.rest {
            return Err(LowerError::Unsupported(format!(
                "rest parameter in {what} (pass explicit arguments instead)"
            )));
        }
        // A default on a destructuring PATTERN (`([x] = pair)`) is rejected
        // precisely; plain defaults (`x = dflt`, either level) become an
        // `undefined`-guarded assign.
        let pat_default = match &p.pattern {
            Pattern::Name { default, .. } => default.clone(),
            _ if p.default.is_some() => {
                return Err(LowerError::Unsupported(format!(
                    "default on a destructuring pattern in {what} (destructure first, then default)"
                )));
            }
            _ => None,
        };
        let default = p.default.clone().or(pat_default);
        match &p.pattern {
            Pattern::Name { name, .. } => {
                names.push(name.clone());
                if let Some(d) = &default {
                    prepend.push(default_assign(name, d, index)?);
                }
            }
            pattern => {
                let synth = format!("$p{i}");
                names.push(synth.clone());
                lower_pattern_into(pattern, &JsExpr::Var(synth), index, &mut prepend)?;
            }
        }
    }
    Ok((names, prepend))
}

/// `name = (name === undefined ? dflt : name)` — parameter defaulting.
fn default_assign(
    name: &str,
    dflt: &r2n_ast::expr::Expr,
    index: &HashMap<String, usize>,
) -> Result<JsExpr, LowerError> {
    Ok(JsExpr::Assign {
        target: Box::new(JsExpr::Var(name.to_string())),
        value: Box::new(JsExpr::If {
            cond: Box::new(JsExpr::Bin {
                op: JsBinOp::StrictEq,
                left: Box::new(JsExpr::Var(name.to_string())),
                right: Box::new(JsExpr::Lit(r2n_ast::lit::Literal::Undefined)),
            }),
            then: Box::new(lower_expr(dflt, index)?),
            else_: Box::new(JsExpr::Var(name.to_string())),
        }),
    })
}

/// All names bound by a pattern (for locals/capture computation).
/// All names bound by a pattern (for linker export surfaces and
/// locals/capture computation). Public so the linker shares one definition.
pub fn pattern_names(pat: &Pattern, out: &mut Vec<String>) {
    match pat {
        Pattern::Name { name, .. } => out.push(name.clone()),
        Pattern::Object { props, rest } => {
            for pr in props {
                if let Some(alias) = &pr.alias {
                    pattern_names(alias, out);
                } else {
                    out.push(pr.key.clone());
                }
            }
            if let Some(r) = rest {
                out.push(r.clone());
            }
        }
        Pattern::Array { items, rest } => {
            for it in items.iter().flatten() {
                pattern_names(it, out);
            }
            if let Some(r) = rest {
                out.push(r.clone());
            }
        }
    }
}

/// Names bound by a binding pattern (destructuring declarations expose every
/// bound name to later statements in the same body). Public for the linker.
pub fn binding_names(pat: &Pattern) -> Vec<String> {
    let mut out = Vec::new();
    pattern_names(pat, &mut out);
    out
}

/// A component-shaped function: `(params, body)` of an `export function`
/// declaration or an inline function expression, unwrapping `memo(...)`
/// (a perf hint — semantically identity). Used by the linker (export
/// surfaces, entry-root fallback) and the lowerer (component lowering):
/// one predicate, shared semantics.
///
/// Matches:
/// - `Expr::Function { params, body }` directly (e.g. `memo(function Item()
///   {...})` unwraps to the inner function), and
/// - any `Call` whose callee is the identifier `memo` with a single
///   function-valued argument (nesting `memo(memo(f))` unwraps fully).
pub fn component_fn_of(expr: &r2n_ast::expr::Expr) -> Option<(&Vec<Param>, &Vec<Stmt>)> {
    match expr {
        r2n_ast::expr::Expr::Function { params, body, .. } => Some((params, body)),
        r2n_ast::expr::Expr::Call { callee, args } => {
            let is_memo = matches!(
                &**callee,
                r2n_ast::expr::Expr::Ident { name, .. } if name == "memo"
            );
            if !is_memo || args.len() != 1 {
                return None;
            }
            let inner = match &args[0] {
                r2n_ast::expr::CallArg::Expr(e) => e,
                r2n_ast::expr::CallArg::Spread(_) => return None,
            };
            component_fn_of(inner)
        }
        _ => None,
    }
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
    // Local names in scope for JSX tag resolution: every param-bound name
    // (destructured params contribute all their names) plus every
    // `let`/`const`/destructuring name declared in the body. Alongside
    // `index`, these let the lowerer decide whether an uppercase `<C/>` is a
    // static component (`index`), a local component VALUE (`locals` →
    // `ReactNode::ComponentExpr`), or genuinely undefined
    // (`UnknownComponent`).
    let mut param_names = Vec::new();
    for p in &c.params {
        pattern_names(&p.pattern, &mut param_names);
    }
    let locals: std::collections::HashSet<String> = param_names
        .iter()
        .cloned()
        .chain(c.body.iter().flat_map(|s| match s {
            Stmt::Let { name, .. } | Stmt::Const { name, .. } => vec![name.clone()],
            Stmt::Destructure { pattern, .. } => binding_names(pattern),
            _ => Vec::new(),
        }))
        .collect();
    // Param defaults (`editing = false`) apply at render time, before the
    // body: the engine binds every param by prop NAME (missing → undefined),
    // and these guarded assigns fill in the defaults.
    for p in &c.params {
        let dflt: Option<(String, r2n_ast::expr::Expr)> = match &p.pattern {
            Pattern::Name { name, default } => p
                .default
                .clone()
                .or(default.clone())
                .map(|d| (name.clone(), d)),
            _ if p.default.is_some() => {
                return Err(LowerError::Unsupported(format!(
                    "default on a destructuring param in component {}",
                    c.name
                )));
            }
            _ => None,
        };
        if let Some((name, d)) = dflt {
            let read = JsExpr::Var(name.clone());
            out.bindings.push((
                name.clone(),
                JsExpr::Assign {
                    target: Box::new(read.clone()),
                    value: Box::new(JsExpr::If {
                        cond: Box::new(JsExpr::Bin {
                            op: JsBinOp::StrictEq,
                            left: Box::new(read.clone()),
                            right: Box::new(JsExpr::Lit(r2n_ast::lit::Literal::Undefined)),
                        }),
                        then: Box::new(lower_expr(&d, index)?),
                        else_: Box::new(read),
                    }),
                },
            ));
        }
    }
    for stmt in &c.body {
        match stmt {
            Stmt::Let { name, value } | Stmt::Const { name, value } => {
                out.bindings.push((name.clone(), lower_expr(value, index)?));
            }
            Stmt::Destructure { pattern, value, .. } => {
                // `const [a, b] = expr;` / `const {k} = expr;` — expand to
                // a `$dstN` temp binding plus one binding per name (bindings
                // evaluate in order into the render env, so temps work).
                let v = lower_expr(value, index)?;
                let mut tmp = Vec::new();
                lower_destructure(pattern, &v, index, &mut tmp)?;
                for e in tmp {
                    match e {
                        JsExpr::Assign { target, value } => match *target {
                            JsExpr::Var(n) => out.bindings.push((n, *value)),
                            other => out.bindings.push((
                                "$stmt".to_string(),
                                JsExpr::Assign {
                                    target: Box::new(other),
                                    value,
                                },
                            )),
                        },
                        other => out.bindings.push(("$stmt".to_string(), other)),
                    }
                }
            }
            // A bare expression statement runs at render time, in source
            // order (this is how `useEffect(fn, deps)` registers).
            Stmt::Expr(e) => out
                .bindings
                .push(("$stmt".to_string(), lower_expr(e, index)?)),
            Stmt::Return(expr) => {
                out.body = lower_renderable(expr, index, &locals)?;
            }
            // General statement grammar (T09b/T10): control flow inside a
            // component render body lowers when the runtime supports it;
            // anything else is a precise error, not a silent miscompile.
            other => {
                return Err(LowerError::Unsupported(format!(
                    "statement in component {} render body: {}",
                    c.name,
                    stmt_kind(other)
                )))
            }
        }
    }
    if !matches!(
        out.body,
        ReactNode::Host { .. }
            | ReactNode::Component { .. }
            | ReactNode::ComponentExpr { .. }
            | ReactNode::If { .. }
            | ReactNode::List { .. }
            | ReactNode::Fragment { .. }
            | ReactNode::ContextProvider { .. }
            | ReactNode::Portal { .. }
            | ReactNode::Suspense { .. }
            | ReactNode::StrictMode { .. }
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
    // Params are the destructured names (the engine binds every param by
    // prop NAME, so `{dispatch}` receives the `dispatch` prop directly).
    let mut final_params = Vec::new();
    for p in &c.params {
        pattern_names(&p.pattern, &mut final_params);
    }
    let local_names: std::collections::HashSet<String> = final_params
        .iter()
        .cloned()
        .chain(out.bindings.iter().map(|(n, _)| n.clone()))
        .collect();
    let captures = free_vars_of_body(&out.bindings, &out.body)
        .into_iter()
        .filter(|n| !local_names.contains(n))
        .collect();
    Ok(RuntimeComponent {
        name: c.name.clone(),
        params: final_params,
        captures,
        bindings: out.bindings,
        body: out.body,
        class: None,
    })
}

/// Short kind name of a statement for precise `Unsupported` errors.
fn stmt_kind(s: &Stmt) -> &'static str {
    match s {
        Stmt::Let { .. } => "let",
        Stmt::Const { .. } => "const",
        Stmt::Destructure { .. } => "destructuring declaration",
        Stmt::Return(_) => "return",
        Stmt::Expr(_) => "expression statement",
        Stmt::If { .. } => "if statement",
        Stmt::While { .. } => "while loop",
        Stmt::For { .. } => "for loop",
        Stmt::Switch { .. } => "switch statement",
        Stmt::Break => "break",
        Stmt::Continue => "continue",
    }
}

/// Lower a class component: `render()` becomes the component body; other
/// methods become `ClassMethod`s; `state = expr` the ClassInfo state.
fn lower_class(
    c: &ClassComponent,
    index: &HashMap<String, usize>,
) -> Result<RuntimeComponent, LowerError> {
    let is_react = c.extends.as_deref() == Some("Component");
    // Non-React (ES) classes are VALUES: `new P(...)` allocates instances
    // with the class's methods on their prototype. No render body.
    if !is_react {
        let state = match &c.state {
            Some(e) => Some(lower_expr(e, index)?),
            None => None,
        };
        let mut methods = Vec::new();
        for m in &c.methods {
            let body_stmts: Vec<JsExpr> = m
                .body
                .iter()
                .map(|st| match st {
                    Stmt::Let { name, value } => Ok(JsExpr::Assign {
                        target: Box::new(JsExpr::Var(name.clone())),
                        value: Box::new(lower_expr(value, index)?),
                    }),
                    Stmt::Const { name, value } => Ok(JsExpr::Assign {
                        target: Box::new(JsExpr::Var(name.clone())),
                        value: Box::new(lower_expr(value, index)?),
                    }),
                    Stmt::Return(expr) => Ok(lower_expr(expr, index)?),
                    Stmt::Expr(e) => Ok(lower_expr(e, index)?),
                    other => Err(LowerError::Unsupported(format!(
                        "statement in ES class method {}: {}",
                        m.name,
                        stmt_kind(other)
                    ))),
                })
                .collect::<Result<_, _>>()?;
            methods.push((
                m.name.clone(),
                ClassMethod {
                    params: plain_param_names(&m.params),
                    body: JsExpr::Block(body_stmts),
                },
            ));
        }
        return Ok(RuntimeComponent {
            name: c.name.clone(),
            params: Vec::new(),
            captures: Vec::new(),
            bindings: Vec::new(),
            body: ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
            class: Some(ClassInfo { state, methods }),
        });
    }
    let mut out = LoweredBody {
        bindings: Vec::new(),
        body: ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
    };
    // `render()` locals for JSX tag resolution (class bodies have no params).
    let locals: std::collections::HashSet<String> = c
        .methods
        .iter()
        .filter(|m| m.name == "render")
        .flat_map(|m| m.body.iter())
        .flat_map(|st| match st {
            Stmt::Let { name, .. } | Stmt::Const { name, .. } => vec![name.clone()],
            Stmt::Destructure { pattern, .. } => binding_names(pattern),
            _ => Vec::new(),
        })
        .collect();
    for m in &c.methods {
        if m.name == "render" {
            for st in &m.body {
                match st {
                    Stmt::Let { name, value } => {
                        out.bindings.push((name.clone(), lower_expr(value, index)?))
                    }
                    Stmt::Const { name, value } => {
                        out.bindings.push((name.clone(), lower_expr(value, index)?))
                    }
                    Stmt::Return(expr) => out.body = lower_renderable(expr, index, &locals)?,
                    Stmt::Expr(e) => out
                        .bindings
                        .push(("$stmt".to_string(), lower_expr(e, index)?)),
                    other => {
                        return Err(LowerError::Unsupported(format!(
                            "statement in class {} render body: {}",
                            c.name,
                            stmt_kind(other)
                        )))
                    }
                }
            }
        }
    }
    // Same renderable validation as function components.
    if !matches!(
        out.body,
        ReactNode::Host { .. }
            | ReactNode::Component { .. }
            | ReactNode::ComponentExpr { .. }
            | ReactNode::If { .. }
            | ReactNode::List { .. }
            | ReactNode::Fragment { .. }
            | ReactNode::ContextProvider { .. }
    ) {
        return Err(LowerError::NonRenderableReturn(format!(
            "class {} renders a non-renderable expression",
            c.name
        )));
    }
    let state = match &c.state {
        Some(e) => Some(lower_expr(e, index)?),
        None => None,
    };
    let mut methods = Vec::new();
    for m in &c.methods {
        if m.name == "render" {
            continue;
        }
        let body_stmts: Vec<JsExpr> = m
            .body
            .iter()
            .map(|st| match st {
                Stmt::Let { name, value } => {
                    // let inside a method: a block binding (assign then value)
                    Ok(JsExpr::Assign {
                        target: Box::new(JsExpr::Var(name.clone())),
                        value: Box::new(lower_expr(value, index)?),
                    })
                }
                Stmt::Const { name, value } => Ok(JsExpr::Assign {
                    target: Box::new(JsExpr::Var(name.clone())),
                    value: Box::new(lower_expr(value, index)?),
                }),
                Stmt::Return(expr) => Ok(lower_expr(expr, index)?),
                Stmt::Expr(e) => Ok(lower_expr(e, index)?),
                other => Err(LowerError::Unsupported(format!(
                    "statement in class method {}: {}",
                    m.name,
                    stmt_kind(other)
                ))),
            })
            .collect::<Result<_, _>>()?;
        methods.push((
            m.name.clone(),
            ClassMethod {
                params: plain_param_names(&m.params),
                body: JsExpr::Block(body_stmts),
            },
        ));
    }
    Ok(RuntimeComponent {
        name: c.name.clone(),
        params: Vec::new(),
        captures: Vec::new(),
        bindings: out.bindings,
        body: out.body,
        class: Some(ClassInfo { state, methods }),
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
        // `undefined` is a keyword, not a variable: it lowers to the
        // literal (a bare undefined reference is an unbound error in our
        // subset otherwise).
        Expr::Ident { name, .. } if name == "undefined" => {
            JsExpr::Lit(r2n_ast::lit::Literal::Undefined)
        }
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
            // (`.get` with a spread arg is a real method call, not an index.)
            if let Expr::Member { base, prop } = &**callee {
                if prop == "get" && args.len() == 1 {
                    if let r2n_ast::expr::CallArg::Expr(key) = &args[0] {
                        return Ok(JsExpr::Index {
                            base: Box::new(lower_expr(base, index)?),
                            key: Box::new(lower_expr(key, index)?),
                        });
                    }
                }
            }
            JsExpr::Call {
                callee: Box::new(lower_expr(callee, index)?),
                args: args
                    .iter()
                    .map(|a| lower_call_arg(a, index))
                    .collect::<Result<_, _>>()?,
            }
        }
        Expr::Array(items) => JsExpr::Array(
            items
                .iter()
                .map(|i| lower_array_item(i, index))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Object(items) => JsExpr::Object(
            items
                .iter()
                .map(|i| lower_object_item(i, index))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Template { parts, exprs } => {
            // `` `a${x}b` `` lowers to string concatenation: the cooked parts
            // are string literals joined with `+` around each interpolation.
            // A template with no interpolations is a single literal.
            let mut acc: Option<JsExpr> = None;
            let push_str = |s: &str, acc: &mut Option<JsExpr>| {
                if s.is_empty() {
                    return;
                }
                let lit = JsExpr::Lit(r2n_ast::lit::Literal::String(s.to_string()));
                *acc = Some(match acc.take() {
                    None => lit,
                    Some(prev) => JsExpr::Bin {
                        op: JsBinOp::Add,
                        left: Box::new(prev),
                        right: Box::new(lit),
                    },
                });
            };
            for (i, part) in parts.iter().enumerate() {
                push_str(part, &mut acc);
                if i < exprs.len() {
                    let e = lower_expr(&exprs[i], index)?;
                    acc = Some(match acc.take() {
                        None => e,
                        Some(prev) => JsExpr::Bin {
                            op: JsBinOp::Add,
                            left: Box::new(prev),
                            right: Box::new(e),
                        },
                    });
                }
            }
            acc.unwrap_or_else(|| JsExpr::Lit(r2n_ast::lit::Literal::String(String::new())))
        }
        Expr::Update { op, target, prefix } => JsExpr::Update {
            inc: matches!(op, r2n_ast::expr::UpdateOp::Inc),
            target: Box::new(lower_expr(target, index)?),
            prefix: *prefix,
        },
        Expr::CompoundAssign { op, target, value } => {
            // `x += v` desugared at parse time to `x = x + v`; the lowerer
            // only ever sees Assign — this arm is unreachable but total.
            JsExpr::Assign {
                target: Box::new(lower_expr(target, index)?),
                value: Box::new(JsExpr::Bin {
                    op: lower_binop(*op),
                    left: Box::new(lower_expr(target, index)?),
                    right: Box::new(lower_expr(value, index)?),
                }),
            }
        }
        Expr::Ternary { cond, then, else_ } => JsExpr::If {
            cond: Box::new(lower_expr(cond, index)?),
            then: Box::new(lower_expr(then, index)?),
            else_: Box::new(lower_expr(else_, index)?),
        },
        Expr::Arrow {
            params,
            body,
            async_,
        } => {
            // Param patterns/defaults bind at call time via prepended
            // assigns (same protocol as plain functions).
            let (names, prepend) = lower_param_binds(params, index, "arrow function")?;
            if *async_ {
                if !prepend.is_empty() {
                    return Err(LowerError::Unsupported(
                        "destructuring/default params on async arrows (use plain params)"
                            .to_string(),
                    ));
                }
                JsExpr::AsyncFn {
                    params: names,
                    segments: lower_async_segments(body, index)?,
                }
            } else {
                // No pattern binds: keep the body unwrapped (stable IR shape
                // for the common plain-params case).
                let lowered = lower_expr(body, index)?;
                let body = if prepend.is_empty() {
                    lowered
                } else {
                    let mut stmts = prepend;
                    stmts.push(lowered);
                    JsExpr::Block(stmts)
                };
                JsExpr::Closure {
                    params: names,
                    captures: vec![], // captures computed lazily by the runtime frame
                    body: Box::new(body),
                }
            }
        }
        Expr::Function { name, params, body } => {
            // A function expression is a closure over a full statement body:
            // statements lower via `lower_stmt`, so early `return`, loops,
            // and `switch` all ride the runtime's control-flow channel
            // (caught at the call boundary, like plain functions).
            let label = name.as_deref().unwrap_or("anonymous function");
            let (names, mut stmts) = lower_param_binds(params, index, label)?;
            for st in body {
                lower_stmt(st, label, index, &mut stmts)?;
            }
            JsExpr::Closure {
                params: names,
                captures: vec![],
                body: Box::new(JsExpr::Block(stmts)),
            }
        }
        Expr::Await { .. } => {
            return Err(LowerError::UnsupportedAwait(
                "await outside an async function".to_string(),
            ))
        }
        Expr::Yield { .. } => {
            return Err(LowerError::UnsupportedAwait(
                "yield outside a generator function".to_string(),
            ))
        }
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
        Expr::New { callee, args } => JsExpr::New {
            callee: Box::new(lower_expr(callee, index)?),
            args: args
                .iter()
                .map(|a| lower_call_arg(a, index))
                .collect::<Result<Vec<_>, _>>()?,
        },
        Expr::Throw(value) => JsExpr::Throw {
            value: Box::new(lower_expr(value, index)?),
        },
        // `return` in block-expression position (try bodies): raises
        // function-return control flow (caught at the call boundary and
        // the async/generator step boundaries).
        Expr::Return(value) => JsExpr::Return(match value {
            Some(v) => Some(Box::new(lower_expr(v, index)?)),
            None => None,
        }),
        Expr::While { cond, body } => JsExpr::While {
            cond: Box::new(lower_expr(cond, index)?),
            body: Box::new(lower_expr(body, index)?),
            step: None,
        },
        // `break`/`continue` in block-expression position lower to the
        // runtime's control-flow channel: the innermost loop/switch driver
        // catches them; a stray use is a precise RUNTIME error naming the
        // construct (total lowering — no silent miscompile).
        Expr::Break => JsExpr::Break,
        Expr::Continue => JsExpr::Continue,
        Expr::Try {
            block,
            catch_param,
            catch,
            finally,
        } => JsExpr::Try {
            block: block
                .iter()
                .map(|s| lower_expr(s, index))
                .collect::<Result<_, _>>()?,
            catch_param: catch_param.clone(),
            catch: catch
                .as_ref()
                .map(|c| {
                    c.iter()
                        .map(|s| lower_expr(s, index))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            finally: finally
                .as_ref()
                .map(|f| {
                    f.iter()
                        .map(|s| lower_expr(s, index))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
        },
        // `import("path")` — dynamic import (M2-T09): lowers to the reserved
        // `@module:` variable, which the runtime evaluates to the module's
        // namespace record. The specifier is a compile-time string literal, so
        // the reserved name is deterministic; the linker (M2-T09) maps it to
        // the module's table index `N` when assembling the program.
        Expr::DynImport { specifier } => JsExpr::Var(format!("@module:{specifier}")),
        Expr::Element(_) => {
            return Err(LowerError::InvalidListMap(
                "element used as a value".to_string(),
            ))
        }
    })
}

/// Lower one call argument: `...spread` becomes a runtime `SpreadArg`
/// (expanded at call time); plain expressions lower directly.
fn lower_call_arg(
    arg: &r2n_ast::expr::CallArg,
    index: &HashMap<String, usize>,
) -> Result<JsExpr, LowerError> {
    match arg {
        r2n_ast::expr::CallArg::Expr(e) => lower_expr(e, index),
        r2n_ast::expr::CallArg::Spread(e) => Ok(JsExpr::SpreadArg(Box::new(lower_expr(e, index)?))),
    }
}

/// Lower one array-literal item.
fn lower_array_item(
    item: &r2n_ast::expr::ArrayItem,
    index: &HashMap<String, usize>,
) -> Result<crate::js::JsArrayItem, LowerError> {
    match item {
        r2n_ast::expr::ArrayItem::Expr(e) => {
            Ok(crate::js::JsArrayItem::Expr(lower_expr(e, index)?))
        }
        r2n_ast::expr::ArrayItem::Spread(e) => {
            Ok(crate::js::JsArrayItem::Spread(lower_expr(e, index)?))
        }
    }
}

/// Lower one object-literal item.
fn lower_object_item(
    item: &r2n_ast::expr::ObjectItem,
    index: &HashMap<String, usize>,
) -> Result<crate::js::JsObjectItem, LowerError> {
    match item {
        r2n_ast::expr::ObjectItem::Shorthand(name) => {
            Ok(crate::js::JsObjectItem::Shorthand(name.clone()))
        }
        r2n_ast::expr::ObjectItem::Prop(k, v) => Ok(crate::js::JsObjectItem::Prop(
            k.clone(),
            lower_expr(v, index)?,
        )),
        r2n_ast::expr::ObjectItem::Spread(e) => {
            Ok(crate::js::JsObjectItem::Spread(lower_expr(e, index)?))
        }
    }
}

/// Lower a top-level `function*` declaration (M2-T08): the body splits into
/// yield-delimited segments (the same machine async fns use — generators are
/// the PULL-based twin: `next()` advances instead of a scheduler).
fn lower_generator_fn(
    g: &r2n_ast::program::GeneratorFn,
    index: &HashMap<String, usize>,
) -> Result<crate::runtime::GeneratorIr, LowerError> {
    // Statements -> expression list (the block model: let/const are Assigns,
    // `return e` is the terminal value; `return yield v` marks the yield).
    let exprs: Vec<Expr> = g
        .body
        .iter()
        .map(|st| match st {
            r2n_ast::program::Stmt::Let { name, value }
            | r2n_ast::program::Stmt::Const { name, value } => Ok(Expr::Assign {
                target: Box::new(Expr::Ident {
                    name: name.clone(),
                    is_component: false,
                }),
                value: Box::new(value.clone()),
            }),
            r2n_ast::program::Stmt::Expr(e) => Ok(e.clone()),
            r2n_ast::program::Stmt::Return(e) => Ok(match e {
                Expr::Yield { value, .. } => Expr::Yield {
                    value: value.clone(),
                    from_return: true,
                },
                other => other.clone(),
            }),
            other => Err(LowerError::Unsupported(format!(
                "statement in generator body: {}",
                stmt_kind(other)
            ))),
        })
        .collect::<Result<_, _>>()?;
    let refs: Vec<&Expr> = exprs.iter().collect();
    let segments = lower_segments(&refs, index, true)?;
    Ok(crate::runtime::GeneratorIr {
        name: g.name.clone(),
        params: plain_param_names(&g.params),
        segments,
    })
}

/// Split a segmented-fn body into await/yield-delimited segments (M2-T07
/// async, M2-T08 generators — the same machine; generators are the
/// PULL-based twin). The supported suspension positions — the real-world
/// surface — are statement values: `let x = await p;` / `x = await p;` /
/// `await p;` / the terminal `return await p;` (yield likewise). A
/// suspension nested inside a larger expression is a precise compile error
/// (a state-machine split of arbitrary expressions would be a CPS
/// transform; the boundary is enforced, not silent).
fn lower_async_segments(
    body: &Expr,
    index: &HashMap<String, usize>,
) -> Result<Vec<crate::js::JsAsyncSegment>, LowerError> {
    let stmts: Vec<&Expr> = match body {
        Expr::Block(ss) => ss.iter().collect(),
        single => vec![single],
    };
    lower_segments(&stmts, index, false)
}

fn lower_segments(
    stmts: &[&Expr],
    index: &HashMap<String, usize>,
    is_generator: bool,
) -> Result<Vec<crate::js::JsAsyncSegment>, LowerError> {
    let kw = if is_generator { "yield" } else { "await" };
    let mut segments = Vec::new();
    let mut cur: Vec<JsExpr> = Vec::new();
    for e in stmts.iter() {
        match e {
            // `x = await p` / `let x = yield v` (the parser lowers let/const
            // to Assign in both arrow blocks and generator bodies).
            Expr::Assign { target, value }
                if matches!(&**value, Expr::Await { .. } | Expr::Yield { .. }) =>
            {
                let bind = match &**target {
                    Expr::Ident { name, .. } => name.clone(),
                    _ => {
                        return Err(LowerError::UnsupportedAwait(format!(
                            "{kw} target must be a plain binding (x = {kw} v), not a member write"
                        )))
                    }
                };
                let v = match &**value {
                    Expr::Await { value: v, .. } | Expr::Yield { value: Some(v), .. } => v,
                    Expr::Yield { .. } => {
                        // `let x = yield;` — value is undefined.
                        &EMPTY_EXPR
                    }
                    _ => unreachable!("matched above"),
                };
                segments.push(crate::js::JsAsyncSegment {
                    stmts: std::mem::take(&mut cur),
                    await_expr: Some(Box::new(lower_expr(v, index)?)),
                    await_bind: Some(bind),
                    await_completes: false,
                });
            }
            // `await p;` / `yield v;` / bare `yield;` — suspend, continue
            // with the next statement. `from_return` (`return await p` /
            // `return yield v`) completes the fn with the incoming value.
            Expr::Await {
                value: v,
                from_return,
            } => {
                segments.push(crate::js::JsAsyncSegment {
                    stmts: std::mem::take(&mut cur),
                    await_expr: Some(Box::new(lower_expr(v, index)?)),
                    await_bind: None,
                    await_completes: *from_return,
                });
            }
            Expr::Yield { value, from_return } => {
                let lowered = match value {
                    Some(v) => lower_expr(v, index)?,
                    None => JsExpr::Lit(r2n_ast::lit::Literal::Undefined),
                };
                segments.push(crate::js::JsAsyncSegment {
                    stmts: std::mem::take(&mut cur),
                    await_expr: Some(Box::new(lowered)),
                    await_bind: None,
                    await_completes: *from_return,
                });
            }
            _ => {
                if contains_await(e) {
                    return Err(LowerError::UnsupportedAwait(format!(
                        "{kw} is only supported as a statement value: let x = {kw} v; | x = {kw} v; | {kw} v; | return {kw} v;"
                    )));
                }
                cur.push(lower_expr(e, index)?);
            }
        }
    }
    // Terminal segment: the fn completes with its last statement's value.
    // (A trailing `return await/yield` never reaches it — the resume
    // settles/completes directly.)
    segments.push(crate::js::JsAsyncSegment {
        stmts: cur,
        await_expr: None,
        await_bind: None,
        await_completes: false,
    });
    Ok(segments)
}

/// Placeholder for `let x = yield;` (yield with no value lowers to
/// undefined). Never evaluated as an expression — the Yield arm above only
/// reaches for it to keep the match exhaustive.
static EMPTY_EXPR: Expr = Expr::Literal(r2n_ast::lit::Literal::Undefined);

/// Does this expression contain an `await`/`yield` OUTSIDE any nested
/// arrow? (A nested arrow's suspensions belong to IT.)
fn contains_await(e: &Expr) -> bool {
    match e {
        Expr::Await { .. } | Expr::Yield { .. } => true,
        Expr::Arrow { .. } => false,
        Expr::Assign { target, value } => contains_await(target) || contains_await(value),
        Expr::Binary { left, right, .. } => contains_await(left) || contains_await(right),
        Expr::Unary { expr, .. } => contains_await(expr),
        Expr::Ternary { cond, then, else_ } => {
            contains_await(cond) || contains_await(then) || contains_await(else_)
        }
        Expr::Call { callee, args } => {
            contains_await(callee)
                || args.iter().any(|a| match a {
                    r2n_ast::expr::CallArg::Expr(e) => contains_await(e),
                    r2n_ast::expr::CallArg::Spread(e) => contains_await(e),
                })
        }
        Expr::New { callee, args } => {
            contains_await(callee)
                || args.iter().any(|a| match a {
                    r2n_ast::expr::CallArg::Expr(e) => contains_await(e),
                    r2n_ast::expr::CallArg::Spread(e) => contains_await(e),
                })
        }
        Expr::Member { base, .. } => contains_await(base),
        Expr::Array(items) => items.iter().any(|i| match i {
            r2n_ast::expr::ArrayItem::Expr(e) => contains_await(e),
            r2n_ast::expr::ArrayItem::Spread(e) => contains_await(e),
        }),
        Expr::Object(items) => items.iter().any(|i| match i {
            r2n_ast::expr::ObjectItem::Shorthand(_) => false,
            r2n_ast::expr::ObjectItem::Prop(_, v) => contains_await(v),
            r2n_ast::expr::ObjectItem::Spread(e) => contains_await(e),
        }),
        Expr::Template { exprs, .. } => exprs.iter().any(contains_await),
        Expr::Update { target, .. } => contains_await(target),
        Expr::CompoundAssign { target, value, .. } => {
            contains_await(target) || contains_await(value)
        }
        Expr::While { cond, body } => contains_await(cond) || contains_await(body),
        Expr::Break | Expr::Continue => false,
        Expr::Block(stmts) => stmts.iter().any(contains_await),
        Expr::Return(v) => v.as_ref().is_some_and(|e| contains_await(e)),
        _ => false,
    }
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
        BinOp::StrictEq => JsBinOp::StrictEq,
        BinOp::StrictNeq => JsBinOp::StrictNeq,
        BinOp::Lt => JsBinOp::Lt,
        BinOp::Gt => JsBinOp::Gt,
        BinOp::Le => JsBinOp::Le,
        BinOp::Ge => JsBinOp::Ge,
        BinOp::And => JsBinOp::And,
        BinOp::Or => JsBinOp::Or,
        BinOp::Nullish => JsBinOp::Nullish,
        BinOp::BitOr => JsBinOp::BitOr,
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
fn lower_renderable(
    expr: &Expr,
    index: &HashMap<String, usize>,
    locals: &std::collections::HashSet<String>,
) -> Result<ReactNode, LowerError> {
    match expr {
        Expr::Element(e) => lower_element(e, index, locals),
        Expr::Ternary { cond, then, else_ } => Ok(ReactNode::If {
            cond: lower_expr(cond, index)?,
            then: Box::new(lower_renderable(then, index, locals)?),
            else_: Box::new(lower_renderable(else_, index, locals)?),
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

fn lower_element(
    e: &Element,
    index: &HashMap<String, usize>,
    locals: &std::collections::HashSet<String>,
) -> Result<ReactNode, LowerError> {
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
            children.push(lower_child(child, index, locals)?);
        }
        return Ok(ReactNode::Fragment { key, children });
    }

    // StrictMode: a transparent marker node (dev double-invoke behavior).
    if e.tag == "StrictMode" {
        let children = e
            .children
            .iter()
            .map(|c| lower_child(c, index, locals))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ReactNode::StrictMode { children });
    }

    // Suspense: `<Suspense fallback={...}>` — a special tag, not a
    // component. `fallback` must be a renderable element.
    if e.tag == "Suspense" {
        let mut fallback = None;
        for p in &e.props {
            if p.name == "fallback" {
                match &p.value {
                    Some(v) => fallback = Some(Box::new(lower_renderable(v, index, locals)?)),
                    None => {
                        return Err(LowerError::NonRenderableReturn(
                            "Suspense fallback must be an element".to_string(),
                        ))
                    }
                }
            }
        }
        let fb = fallback.ok_or_else(|| {
            LowerError::NonRenderableReturn("Suspense requires fallback".to_string())
        })?;
        let children = e
            .children
            .iter()
            .map(|c| lower_child(c, index, locals))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ReactNode::Suspense {
            fallback: fb,
            children,
        });
    }

    // Portal: `<Portal target="class">` — a special tag, not a component.
    if e.tag == "Portal" {
        let mut target = String::new();
        for p in &e.props {
            if p.name == "target" {
                target = match &p.value {
                    Some(Expr::Literal(r2n_ast::lit::Literal::String(s))) => s.clone(),
                    _ => {
                        return Err(LowerError::NonRenderableReturn(
                            "Portal target must be a string literal".to_string(),
                        ))
                    }
                };
            }
        }
        if target.is_empty() {
            return Err(LowerError::NonRenderableReturn(
                "Portal requires a `target` className".to_string(),
            ));
        }
        let children = e
            .children
            .iter()
            .map(|c| lower_child(c, index, locals))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ReactNode::Portal { target, children });
    }

    // Context provider: `<Ctx.Provider value={...}>` — the dotted tag names
    // the context handle's Provider member, not a component table entry.
    if let Some((base, member)) = e.tag.rsplit_once('.') {
        if member == "Provider" && !base.is_empty() {
            let ctx = JsExpr::Var(base.to_string());
            let mut value = JsExpr::Lit(r2n_ast::lit::Literal::Null);
            for p in &e.props {
                if p.name == "value" {
                    value = match &p.value {
                        Some(v) => lower_expr(v, index)?,
                        None => JsExpr::Lit(r2n_ast::lit::Literal::Bool(true)),
                    };
                }
            }
            let children = e
                .children
                .iter()
                .map(|c| lower_child(c, index, locals))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(ReactNode::ContextProvider {
                ctx,
                value,
                children,
            });
        }
    }

    // Component element? (uppercase tag)
    if e.is_component {
        let mut props = lower_props(&e.props, index)?;
        // React children composition: JSX children of a component element
        // become its `children` prop. They are lowered NOW, in the parent's
        // context, and carried as pre-built React-IR nodes (a `Value::Children`
        // at runtime) — the child splices them via `ReactNode::Children`.
        if !e.children.is_empty() {
            let mut children = Vec::with_capacity(e.children.len());
            for child in &e.children {
                children.push(lower_child(child, index, locals)?);
            }
            props.push(("children".to_string(), JsExpr::Children(children)));
        }
        // `<ns.X/>` — a member-access component tag. The base must be a LOCAL
        // (param/let/const) bound to a module namespace or component holder;
        // the member is resolved at RENDER time off that value (e.g.
        // `const ns = import("widget"); return <ns.Widget/>;`, or a namespace
        // object passed as a prop and rendered `<P.Widget/>`). A dotted tag is
        // never a static table entry, so it lowers to a `ReactNode::ComponentExpr`
        // whose component is a member access evaluated in the parent scope.
        if let Some((base, member)) = e.tag.rsplit_once('.') {
            return if !base.is_empty() && locals.contains(base) {
                Ok(ReactNode::ComponentExpr {
                    component: JsExpr::Get {
                        base: Box::new(JsExpr::Var(base.to_string())),
                        prop: member.to_string(),
                    },
                    props,
                })
            } else {
                Err(LowerError::UnknownComponent(e.tag.clone()))
            };
        }
        // Static component: an uppercase tag naming a component in `index`
        // (own declarations and imported bindings) — a fixed table id, so it
        // lowers once at build time to a `ReactNode::Component`.
        if let Some(&comp_idx) = index.get(&e.tag) {
            return Ok(ReactNode::Component {
                component: ComponentRef(comp_idx),
                props,
            });
        }
        // `<C>` where `C` is a LOCAL value (a `let`/`const`/param) bound to a
        // component reference (e.g. `const C = m.Widget; return <C/>;`, or a
        // component passed as a prop and rendered as `<P/>`). The identity is a
        // RUNTIME value, so it cannot be a static table id: emit a
        // `ReactNode::ComponentExpr` the engine resolves against the scope.
        if locals.contains(&e.tag) {
            return Ok(ReactNode::ComponentExpr {
                component: JsExpr::Var(e.tag.clone()),
                props,
            });
        }
        return Err(LowerError::UnknownComponent(e.tag.clone()));
    }

    // Host element. Detect the special `children.map(...)` list form: when the
    // sole child is a `.map` call whose arrow body is a JSX element, we model it
    // as a keyed `ReactNode::List` nested inside the host's children (the
    // runtime flattens it into the host's child list at render time).
    if e.children.len() == 1 {
        if let Some(list) = try_lower_list(&e.children[0], index, locals)? {
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
        children.push(lower_child(child, index, locals)?);
    }
    Ok(ReactNode::Host {
        tag: e.tag.clone(),
        props,
        children,
    })
}

/// Lower a child expression node (text/element/conditional/list).
fn lower_child(
    child: &Expr,
    index: &HashMap<String, usize>,
    locals: &std::collections::HashSet<String>,
) -> Result<ReactNode, LowerError> {
    match child {
        Expr::Element(_) => lower_renderable(child, index, locals),
        Expr::Ternary { .. } => lower_renderable(child, index, locals),
        // `children` as a host element's child: the splice point.
        Expr::Ident {
            name,
            is_component: false,
        } if name == "children" => Ok(ReactNode::Children),
        Expr::Call { .. } => {
            // possible `items.map(x => <li/>)` directly as a child
            if let Some(list) = try_lower_list(child, index, locals)? {
                Ok(list)
            } else {
                // Any other call (`useContext(Ctx)`, `arr[i]`, ...) is a
                // VALUE rendered as text — the call evaluates at render.
                Ok(ReactNode::Text(lower_expr(child, index)?))
            }
        }
        // `{cond && <el/>}` / `{cond || <el/>}`: short-circuit rendering.
        Expr::Binary { .. } if is_short_circuit_render(child) => {
            lower_short_circuit(child, index, locals)
        }
        // A `{expr}` child renders as text.
        other => Ok(ReactNode::Text(lower_expr(other, index)?)),
    }
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
    locals: &std::collections::HashSet<String>,
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
    let node = lower_renderable(node_side, index, locals)?;
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
    locals: &std::collections::HashSet<String>,
) -> Result<Option<ReactNode>, LowerError> {
    // Pattern: Call { callee: Member { base, prop: "map" }, args: [arrow] }
    let (base, arrow) = match expr {
        Expr::Call { callee, args } if args.len() == 1 => match &**callee {
            Expr::Member { base, prop } if prop == "map" => (base, &args[0]),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let arrow = match arrow {
        r2n_ast::expr::CallArg::Expr(e) => e,
        r2n_ast::expr::CallArg::Spread(_) => return Ok(None),
    };
    let (params, arrow_body) = match arrow {
        Expr::Arrow { params, body, .. } => (params, body),
        _ => return Ok(None),
    };
    // The arrow body must be a JSX element (so each item becomes a node).
    let item = match &**arrow_body {
        Expr::Element(_) => lower_renderable(arrow_body, index, locals)?,
        _ => return Ok(None),
    };
    // The per-element variable name is the arrow's first PLAIN parameter.
    // (Destructuring `.map(({a}) => ...)` lowers the arrow normally and
    // skips the List fast path — correct, not silent.)
    let item_var = params
        .first()
        .and_then(|p| match &p.pattern {
            Pattern::Name { name, .. } => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "$item".to_string());
    // Rewrite every occurrence of `item_var` in the item tree and key to the
    // runtime's reserved name `$item`, so the runtime can substitute the actual
    // element value at render time.
    let item = subst_node(item, &item_var, "$item");
    // The key expression: prefer the `key` prop of the item element; otherwise
    // the item value itself.
    let key_expr = match &item {
        ReactNode::Host { props, .. }
        | ReactNode::Component { props, .. }
        | ReactNode::ComponentExpr { props, .. } => {
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
        JsExpr::SpreadArg(e) => JsExpr::SpreadArg(Box::new(subst_expr(*e, from, to))),
        JsExpr::New { callee, args } => JsExpr::New {
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
        JsExpr::Array(items) => JsExpr::Array(
            items
                .into_iter()
                .map(|i| match i {
                    crate::js::JsArrayItem::Expr(e) => {
                        crate::js::JsArrayItem::Expr(subst_expr(e, from, to))
                    }
                    crate::js::JsArrayItem::Spread(e) => {
                        crate::js::JsArrayItem::Spread(subst_expr(e, from, to))
                    }
                })
                .collect(),
        ),
        JsExpr::Object(items) => JsExpr::Object(
            items
                .into_iter()
                .map(|i| match i {
                    crate::js::JsObjectItem::Shorthand(n) => {
                        if n == from {
                            crate::js::JsObjectItem::Shorthand(to.to_string())
                        } else {
                            crate::js::JsObjectItem::Shorthand(n)
                        }
                    }
                    crate::js::JsObjectItem::Prop(k, v) => {
                        crate::js::JsObjectItem::Prop(k, subst_expr(v, from, to))
                    }
                    crate::js::JsObjectItem::Spread(e) => {
                        crate::js::JsObjectItem::Spread(subst_expr(e, from, to))
                    }
                })
                .collect(),
        ),
        JsExpr::While { cond, body, step } => JsExpr::While {
            cond: Box::new(subst_expr(*cond, from, to)),
            body: Box::new(subst_expr(*body, from, to)),
            step: step.map(|s| Box::new(subst_expr(*s, from, to))),
        },
        JsExpr::Switch {
            disc,
            cases,
            default,
        } => JsExpr::Switch {
            disc: Box::new(subst_expr(*disc, from, to)),
            cases: cases
                .into_iter()
                .map(|c| crate::js::SwitchCase {
                    test: subst_expr(c.test, from, to),
                    body: c
                        .body
                        .into_iter()
                        .map(|s| subst_expr(s, from, to))
                        .collect(),
                })
                .collect(),
            default: default.map(|d| d.into_iter().map(|s| subst_expr(s, from, to)).collect()),
        },
        JsExpr::Break => JsExpr::Break,
        JsExpr::Continue => JsExpr::Continue,
        JsExpr::Return(v) => JsExpr::Return(v.map(|e| Box::new(subst_expr(*e, from, to)))),
        JsExpr::Update {
            inc,
            target,
            prefix,
        } => JsExpr::Update {
            inc,
            target: Box::new(subst_expr(*target, from, to)),
            prefix,
        },
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
        JsExpr::AsyncFn { params, segments } => JsExpr::AsyncFn {
            params,
            segments: segments
                .into_iter()
                .map(|mut s| {
                    s.stmts = s
                        .stmts
                        .into_iter()
                        .map(|e| subst_expr(e, from, to))
                        .collect();
                    s.await_expr = s.await_expr.map(|a| Box::new(subst_expr(*a, from, to)));
                    s
                })
                .collect(),
        },
        JsExpr::Throw { value } => JsExpr::Throw {
            value: Box::new(subst_expr(*value, from, to)),
        },
        JsExpr::Try {
            block,
            catch_param,
            catch,
            finally,
        } => {
            // A catch param named `from` shadows the substitution inside the
            // catch body (ECMA scoping) — decide before `catch_param` moves.
            let shadows = catch_param.as_deref() == Some(from);
            JsExpr::Try {
                block: block.into_iter().map(|s| subst_expr(s, from, to)).collect(),
                // A catch param named `from` shadows the substitution inside
                // the catch body (ECMA scoping).
                catch_param,
                catch: if shadows {
                    catch
                } else {
                    catch.map(|c| c.into_iter().map(|s| subst_expr(s, from, to)).collect())
                },
                finally: finally.map(|f| f.into_iter().map(|s| subst_expr(s, from, to)).collect()),
            }
        }
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
        ReactNode::ComponentExpr { component, props } => ReactNode::ComponentExpr {
            component: subst_expr(component, from, to),
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
        ReactNode::ContextProvider {
            ctx,
            value,
            children,
        } => ReactNode::ContextProvider {
            ctx: subst_expr(ctx, from, to),
            value: subst_expr(value, from, to),
            children: children
                .into_iter()
                .map(|c| subst_node(c, from, to))
                .collect(),
        },
        ReactNode::StrictMode { children } => ReactNode::StrictMode {
            children: children
                .into_iter()
                .map(|c| subst_node(c, from, to))
                .collect(),
        },
        ReactNode::Suspense { fallback, children } => ReactNode::Suspense {
            fallback: Box::new(subst_node(*fallback, from, to)),
            children: children
                .into_iter()
                .map(|c| subst_node(c, from, to))
                .collect(),
        },
        ReactNode::Portal { target, children } => ReactNode::Portal {
            target,
            children: children
                .into_iter()
                .map(|c| subst_node(c, from, to))
                .collect(),
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
        JsExpr::New { callee, args } => {
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
                match i {
                    crate::js::JsArrayItem::Expr(e) => collect_free(e, bound, out),
                    crate::js::JsArrayItem::Spread(e) => collect_free(e, bound, out),
                }
            }
        }
        JsExpr::Object(items) => {
            for i in items {
                match i {
                    crate::js::JsObjectItem::Shorthand(n) => {
                        if !bound.contains(n) && !out.contains(n) {
                            out.push(n.clone());
                        }
                    }
                    crate::js::JsObjectItem::Prop(_, v) => collect_free(v, bound, out),
                    crate::js::JsObjectItem::Spread(e) => collect_free(e, bound, out),
                }
            }
        }
        JsExpr::While { cond, body, step } => {
            collect_free(cond, bound, out);
            collect_free(body, bound, out);
            if let Some(s) = step {
                collect_free(s, bound, out);
            }
        }
        JsExpr::Switch {
            disc,
            cases,
            default,
        } => {
            collect_free(disc, bound, out);
            for c in cases {
                collect_free(&c.test, bound, out);
                for s in &c.body {
                    collect_free(s, bound, out);
                }
            }
            if let Some(d) = default {
                for s in d {
                    collect_free(s, bound, out);
                }
            }
        }
        JsExpr::Break | JsExpr::Continue => {}
        JsExpr::Return(v) => {
            if let Some(e) = v {
                collect_free(e, bound, out);
            }
        }
        JsExpr::Update { target, .. } => collect_free(target, bound, out),
        JsExpr::SpreadArg(e) => collect_free(e, bound, out),
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
        JsExpr::AsyncFn { params, segments } => {
            let mut inner = bound.clone();
            for p in params {
                inner.insert(p.clone());
            }
            for s in segments {
                for st in &s.stmts {
                    collect_free(st, &inner, out);
                }
                if let Some(a) = &s.await_expr {
                    collect_free(a, &inner, out);
                }
            }
        }
        JsExpr::Throw { value } => collect_free(value, bound, out),
        JsExpr::Try {
            block,
            catch_param,
            catch,
            finally,
        } => {
            for s in block {
                collect_free(s, bound, out);
            }
            if let Some(c) = catch {
                // The catch param is a binding: it shadows outer names.
                let mut inner = bound.clone();
                if let Some(p) = catch_param {
                    inner.insert(p.clone());
                }
                for s in c {
                    collect_free(s, &inner, out);
                }
            }
            if let Some(f) = finally {
                for s in f {
                    collect_free(s, bound, out);
                }
            }
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
        ReactNode::ComponentExpr { component, props } => {
            collect_free(component, bound, out);
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
        ReactNode::ContextProvider {
            ctx,
            value,
            children,
        } => {
            collect_free(ctx, bound, out);
            collect_free(value, bound, out);
            for c in children {
                collect_free_node(c, bound, out);
            }
        }
        ReactNode::StrictMode { children } => {
            for c in children {
                collect_free_node(c, bound, out);
            }
        }
        ReactNode::Suspense { fallback, children } => {
            collect_free_node(fallback, bound, out);
            for c in children {
                collect_free_node(c, bound, out);
            }
        }
        ReactNode::Portal { children, .. } => {
            for c in children {
                collect_free_node(c, bound, out);
            }
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
