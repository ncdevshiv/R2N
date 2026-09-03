//! Multi-module linking (M2-T09): assemble an entry source and every module it
//! (transitively) statically imports into ONE `RuntimeTemplate`.
//!
//! Every component lives in ONE global table; the linker discovers the reachable
//! module graph, flattens it, and resolves cross-module `import`/`export`
//! bindings to GLOBAL component indices so the runtime sees a single flat table.
//! Modules are deduplicated by canonical id; a cycle is a precise link error;
//! component-table order follows discovery order (deterministic artifacts).
//! Dynamic `import("path")` is discovered by an AST walk, so a module reachable
//! ONLY dynamically is still linked, laid out, and bound as a namespace; its
//! specifier is canonicalized to the resolved module id so the runtime's
//! `@module:{id}` key matches where the compiler emits it.

use r2n_ast::expr::Expr;
use r2n_ast::program::{Decl, Import, Program, Stmt};
use r2n_ir::react::ReactNode;
use r2n_ir::runtime::{FuncIr, GeneratorIr, ModuleIr, RuntimeComponent, RuntimeTemplate};
use r2n_ir::{component_fn_of, js::JsExpr, lower_module_parts, pattern_names};
use r2n_parser::Parser;
use std::collections::{BTreeSet, HashMap};
use std::fmt;

/// A link-time error.
#[derive(Debug, Clone)]
pub enum LinkError {
    /// A module specifier could not be resolved to a canonical id.
    Resolve {
        from: String,
        specifier: String,
        reason: String,
    },
    /// A module's source could not be loaded.
    Load(String),
    /// A module failed to parse.
    Parse(String),
    /// A module failed to lower.
    Lower(String),
    /// The module graph contains an import cycle.
    ImportCycle(Vec<String>),
    /// The entry module does not `export default` a component.
    NoDefault(String),
    /// An import binding references an export the target never declares.
    UnknownExport { module: String, exported: String },
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Resolve {
                from,
                specifier,
                reason,
            } => {
                write!(
                    f,
                    "cannot resolve '{specifier}' imported by '{from}': {reason}"
                )
            }
            LinkError::Load(m) => write!(f, "load error: {m}"),
            LinkError::Parse(m) => write!(f, "parse error: {m}"),
            LinkError::Lower(m) => write!(f, "lower error: {m}"),
            LinkError::ImportCycle(path) => write!(f, "import cycle: {}", path.join(" -> ")),
            LinkError::NoDefault(m) => write!(f, "no default export: {m}"),
            LinkError::UnknownExport { module, exported } => {
                write!(f, "module '{module}' has no export '{exported}'")
            }
        }
    }
}

impl std::error::Error for LinkError {}

impl From<r2n_parser::ParseError> for LinkError {
    fn from(e: r2n_parser::ParseError) -> Self {
        LinkError::Parse(e.to_string())
    }
}

impl From<r2n_ir::LowerError> for LinkError {
    fn from(e: r2n_ir::LowerError) -> Self {
        LinkError::Lower(e.to_string())
    }
}

/// Resolves module specifiers to canonical ids and loads module source.
pub trait ModuleResolver {
    /// Resolve `specifier` (as written in an `import` in module `from_id`) to a
    /// canonical, unique module id, or fail with a resolve error.
    fn resolve(&self, specifier: &str, from_id: &str) -> Result<String, LinkError>;
    /// Load the source of a module previously returned by `resolve`.
    fn load(&self, id: &str) -> Result<String, LinkError>;
}

/// A filesystem-backed resolver. Module ids are absolute paths; a specifier
/// resolves relative to its importing module's directory with a `.r2n`
/// extension appended when the specifier has no extension. Ids are lexically
/// normalized (`.`/`..` resolved) without touching the filesystem, so they are
/// deterministic and free of the `\\?\` prefix Windows `canonicalize` adds.
pub struct FsResolver {
    pub root: std::path::PathBuf,
}

impl FsResolver {
    /// A resolver rooted at `root` (the base for relative entry ids).
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

/// True when a module specifier is EXTERNAL (no file to load): a bare
/// package name (`react`, `classnames`, `react-router-dom`, possibly with a
/// subpath) or a stylesheet side-effect import (`*.css`). External value
/// imports (`useState`, `memo`, `classnames`, `useLocation`) resolve at
/// RUNTIME by builtin name — the linker skips them entirely (no DFS edge,
/// no export-surface check).
pub fn is_external_specifier(spec: &str) -> bool {
    if spec.ends_with(".css") {
        return true;
    }
    // Relative/absolute paths are always internal (probed with extensions).
    if spec.starts_with('.') || spec.starts_with('/') {
        return false;
    }
    // Windows drive paths (`C:/...`) are internal; everything else bare is a
    // package name.
    if spec.len() >= 2 && spec.as_bytes()[1] == b':' {
        return false;
    }
    true
}

impl ModuleResolver for FsResolver {
    fn resolve(&self, specifier: &str, from_id: &str) -> Result<String, LinkError> {
        if is_external_specifier(specifier) {
            return Err(LinkError::Resolve {
                from: from_id.to_string(),
                specifier: specifier.to_string(),
                reason: "external package (no module file; imports resolve as runtime builtins)"
                    .to_string(),
            });
        }
        let base = std::path::Path::new(from_id)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(self.root.as_path());
        let path = base.join(specifier);
        // Extension probing: exact path first, then `.r2n` (native), `.js`,
        // `.jsx` (real-world React sources). First EXISTING file wins, so
        // `import "./reducer"` finds `reducer.js` and `import "./input"`
        // finds `input.jsx`.
        if path.extension().is_some() {
            return Ok(normalize_path(&path.display().to_string()));
        }
        for ext in ["r2n", "js", "jsx"] {
            let mut candidate = path.clone();
            candidate.set_extension(ext);
            if candidate.exists() {
                return Ok(normalize_path(&candidate.display().to_string()));
            }
        }
        // Nothing on disk: default to `.r2n` so the load error names the
        // native expectation (deterministic, debuggable).
        let mut fallback = path.clone();
        fallback.set_extension("r2n");
        Ok(normalize_path(&fallback.display().to_string()))
    }

    fn load(&self, id: &str) -> Result<String, LinkError> {
        std::fs::read_to_string(id).map_err(|e| LinkError::Load(format!("{id}: {e}")))
    }
}

/// An in-memory resolver for tests/hosts with no filesystem.
#[derive(Default)]
pub struct MemResolver {
    files: HashMap<String, String>,
}

impl MemResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a module source under a canonical id.
    pub fn add(&mut self, id: impl Into<String>, source: impl Into<String>) -> &mut Self {
        self.files.insert(id.into(), source.into());
        self
    }
}

impl ModuleResolver for MemResolver {
    fn resolve(&self, specifier: &str, from_id: &str) -> Result<String, LinkError> {
        // The raw specifier first (tests/hosts may key modules by exactly it).
        if self.files.contains_key(specifier) {
            return Ok(specifier.to_string());
        }
        // Then the lexically normalized form (handles `.`/`..`), then the
        // base-join relative to the importing module's directory — mirroring
        // FsResolver so relative `import("./widget")` resolves deterministically.
        let norm = normalize_path(specifier);
        if self.files.contains_key(&norm) {
            return Ok(norm.clone());
        }
        let dir = from_id.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let candidate = if dir.is_empty() {
            norm
        } else {
            normalize_path(&format!("{dir}/{specifier}"))
        };
        if self.files.contains_key(&candidate) {
            return Ok(candidate);
        }
        Err(LinkError::Resolve {
            from: from_id.to_string(),
            specifier: specifier.to_string(),
            reason: "no such module in resolver".to_string(),
        })
    }

    fn load(&self, id: &str) -> Result<String, LinkError> {
        self.files
            .get(id)
            .cloned()
            .ok_or_else(|| LinkError::Load(format!("no source for module '{id}'")))
    }
}

/// Lexically normalize a path: collapse `.`/`..`, unify separators, preserving
/// the leading `..` segments. Deterministic; does not touch the filesystem.
fn normalize_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                if let Some(last) = parts.last() {
                    if *last != ".." {
                        parts.pop();
                        continue;
                    }
                }
                parts.push("..");
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return ".".to_string();
    }
    parts.join("/")
}

/// The kind of a module-level declaration (informs how a binding is used).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportKind {
    Component,
    Generator,
    /// A plain `function` value, a `const` binding, or a top-level `let`.
    /// These live in the module namespace record (runtime global env), not
    /// in the component table.
    Value,
}

/// A resolved export: how it is used and its GLOBAL index (component only).
#[derive(Debug, Clone, Copy)]
struct Export {
    kind: ExportKind,
    index: usize,
}

/// A parsed module plus the canonical ids it statically imports.
struct Module {
    id: String,
    program: Program,
    /// Canonical ids of statically imported dependencies, in source order.
    deps: Vec<String>,
}

/// Link an entry source into a single production `RuntimeTemplate`, resolving
/// every module it statically imports (transitively) into one global table.
pub fn link_source(
    entry_source: &str,
    entry_id: &str,
    resolver: &dyn ModuleResolver,
) -> Result<RuntimeTemplate, LinkError> {
    link_source_mode(entry_source, entry_id, resolver, false)
}

/// Dev-mode link: keeps StrictMode nodes and marks the artifact.
pub fn link_source_dev(
    entry_source: &str,
    entry_id: &str,
    resolver: &dyn ModuleResolver,
) -> Result<RuntimeTemplate, LinkError> {
    link_source_mode(entry_source, entry_id, resolver, true)
}

fn link_source_mode(
    entry_source: &str,
    entry_id: &str,
    resolver: &dyn ModuleResolver,
    dev: bool,
) -> Result<RuntimeTemplate, LinkError> {
    // 1. Discover the module graph (DFS from the entry), detecting cycles.
    let mut modules: HashMap<String, Module> = HashMap::new();
    let mut order: Vec<String> = Vec::new(); // discovery order, entry first
    let mut visiting: Vec<String> = Vec::new(); // DFS stack (cycle detection)
    let mut visited: BTreeSet<String> = BTreeSet::new();

    fn dfs(
        id: &str,
        source: &str,
        resolver: &dyn ModuleResolver,
        modules: &mut HashMap<String, Module>,
        order: &mut Vec<String>,
        visiting: &mut Vec<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), LinkError> {
        if visited.contains(id) {
            return Ok(());
        }
        if let Some(pos) = visiting.iter().position(|v| v == id) {
            let mut cycle: Vec<String> = visiting[pos..].to_vec();
            cycle.push(id.to_string());
            return Err(LinkError::ImportCycle(cycle));
        }
        visiting.push(id.to_string());
        let mut program = parse_module(source)?;
        let mut deps = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for spec in static_import_specifiers(&program) {
            // External packages and stylesheet side-effects contribute no
            // module: a resolver reports them as a resolve error, which we
            // skip here (their names resolve at runtime as builtins, or —
            // for CSS — not at all). Any OTHER resolve failure propagates.
            // (A resolver that KNOWS a bare id — MemResolver test fixtures —
            // returns Ok, so fixture modules still link.)
            let dep = match resolver.resolve(&spec, id) {
                Ok(dep) => dep,
                Err(_) if is_external_specifier(&spec) => continue,
                Err(e) => return Err(e),
            };
            if seen.insert(dep.clone()) {
                deps.push(dep);
            }
        }
        // Dynamic `import("path")` (M2-T09): also discover so a module reachable
        // ONLY dynamically is still linked, laid out, and bound as a namespace.
        // Specifiers are canonicalized to their resolved ids in the post-pass.
        let mut dyn_specs = Vec::new();
        for_each_dyn_import_in_program(&mut program, &mut |s| dyn_specs.push(s.clone()));
        for spec in &dyn_specs {
            let dep = match resolver.resolve(spec, id) {
                Ok(dep) => dep,
                Err(_) if is_external_specifier(spec) => continue,
                Err(e) => return Err(e),
            };
            if seen.insert(dep.clone()) {
                deps.push(dep);
            }
        }
        // PRE-order: register the module before descending so the component
        // table is laid out entry-first (deterministic and intuitive).
        modules.insert(
            id.to_string(),
            Module {
                id: id.to_string(),
                program,
                deps,
            },
        );
        order.push(id.to_string());
        let stored = modules[id].deps.clone();
        for dep in &stored {
            let dep_source = resolver.load(dep)?;
            dfs(
                dep,
                &dep_source,
                resolver,
                modules,
                order,
                visiting,
                visited,
            )?;
        }
        visiting.pop();
        visited.insert(id.to_string());
        Ok(())
    }

    dfs(
        entry_id,
        entry_source,
        resolver,
        &mut modules,
        &mut order,
        &mut visiting,
        &mut visited,
    )?;

    // Canonicalize dynamic-import specifiers: rewrite each `import("raw")` to
    // `import("canonical_id")` so the lowerer's `@module:{specifier}` key matches
    // the module namespace the runtime binds (`@module:{module.id}`). Discovery
    // already verified every dynamic specifier resolves, so re-resolution here
    // cannot fail.
    for (id, module) in modules.iter_mut() {
        for_each_dyn_import_in_program(&mut module.program, &mut |s| {
            if let Ok(canon) = resolver.resolve(s, id) {
                *s = canon;
            }
        });
    }

    // 2. Assign a global component index to every module's own declarations
    //    (discovery order = deterministic table layout). `component`/`class`
    //    decls are components; so is every `export function Name()` (React
    //    semantics: an exported function returning JSX is a component) and
    //    every `export const Name = memo(function...)` (same, through the
    //    memo HOF — semantically identity).
    let mut bases: HashMap<String, usize> = HashMap::new();
    let mut next = 0usize;
    let mut own_name: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for id in &order {
        let m = &modules[id];
        bases.insert(id.clone(), next);
        let mut local: HashMap<String, usize> = HashMap::new();
        for decl in &m.program.decls {
            match decl {
                Decl::Component(c) => {
                    local.insert(c.name.clone(), next);
                    next += 1;
                }
                Decl::Class(c) => {
                    local.insert(c.name.clone(), next);
                    next += 1;
                }
                Decl::ExportDecl(r2n_ast::program::ExportDecl::Function(f)) => {
                    local.insert(f.name.clone(), next);
                    next += 1;
                }
                Decl::ExportDecl(r2n_ast::program::ExportDecl::Const { name, value })
                    if component_fn_of(value).is_some() =>
                {
                    local.insert(name.clone(), next);
                    next += 1;
                }
                _ => {}
            }
        }
        own_name.insert(id.clone(), local);
    }
    let total_components = next;

    // 3. Build each module's EXPORT surface: exported name -> Export. A module
    //    exports ONLY what it declares explicitly (`export default Name` and
    //    `export { a, b as c }`); declared components are NOT implicitly
    //    exported. The reserved "default" entry maps to the default decl.
    let mut exports: HashMap<String, HashMap<String, Export>> = HashMap::new();
    for id in &order {
        let m = &modules[id];
        let own = &own_name[id];
        // Every module-level declaration, keyed by name (for explicit exports).
        let mut decl_map: HashMap<String, Export> = HashMap::new();
        for decl in &m.program.decls {
            match decl {
                Decl::Component(c) => {
                    let idx = own[&c.name];
                    decl_map.insert(
                        c.name.clone(),
                        Export {
                            kind: ExportKind::Component,
                            index: idx,
                        },
                    );
                }
                Decl::Class(c) => {
                    let idx = own[&c.name];
                    decl_map.insert(
                        c.name.clone(),
                        Export {
                            kind: ExportKind::Component,
                            index: idx,
                        },
                    );
                }
                Decl::GeneratorFn(g) => {
                    decl_map.insert(
                        g.name.clone(),
                        Export {
                            kind: ExportKind::Generator,
                            index: usize::MAX,
                        },
                    );
                }
                Decl::FuncDecl(f) => {
                    decl_map.insert(
                        f.name.clone(),
                        Export {
                            kind: ExportKind::Value,
                            index: usize::MAX,
                        },
                    );
                }
                Decl::TopLevel { pattern, .. } => {
                    let mut names = Vec::new();
                    pattern_names(pattern, &mut names);
                    for name in names {
                        decl_map.insert(
                            name,
                            Export {
                                kind: ExportKind::Value,
                                index: usize::MAX,
                            },
                        );
                    }
                }
                Decl::ExportDecl(e) => match e {
                    r2n_ast::program::ExportDecl::Function(f) => {
                        // Exported functions are COMPONENTS (React semantics):
                        // the index was pre-assigned in step 2.
                        let idx = own[&f.name];
                        decl_map.insert(
                            f.name.clone(),
                            Export {
                                kind: ExportKind::Component,
                                index: idx,
                            },
                        );
                    }
                    r2n_ast::program::ExportDecl::Const { name, value } => {
                        // `export const Name = memo(function...)` is a
                        // component (through the memo HOF); any other const
                        // is a module value (global env at runtime).
                        match own.get(name) {
                            Some(idx) => {
                                decl_map.insert(
                                    name.clone(),
                                    Export {
                                        kind: ExportKind::Component,
                                        index: *idx,
                                    },
                                );
                            }
                            None => {
                                debug_assert!(component_fn_of(value).is_none());
                                decl_map.insert(
                                    name.clone(),
                                    Export {
                                        kind: ExportKind::Value,
                                        index: usize::MAX,
                                    },
                                );
                            }
                        }
                    }
                },
                _ => {}
            }
        }
        let mut map: HashMap<String, Export> = HashMap::new();
        for decl in &m.program.decls {
            match decl {
                Decl::ExportDefault(name) => {
                    if let Some(e) = decl_map.get(name) {
                        map.insert("default".to_string(), *e);
                    }
                }
                Decl::ExportNamed(names) => {
                    for (local, exported) in &names.names {
                        if let Some(e) = decl_map.get(local) {
                            map.insert(exported.clone(), *e);
                        }
                    }
                }
                Decl::ExportDecl(e) => match e {
                    r2n_ast::program::ExportDecl::Function(f) => {
                        if let Some(x) = decl_map.get(&f.name) {
                            map.insert(f.name.clone(), *x);
                        }
                    }
                    r2n_ast::program::ExportDecl::Const { name, .. } => {
                        if let Some(x) = decl_map.get(name) {
                            map.insert(name.clone(), *x);
                        }
                    }
                },
                _ => {}
            }
        }
        exports.insert(id.clone(), map);
    }

    // 4. Lower every module with a `names` map: own declarations + imported
    //    component/class bindings -> global component index.
    let mut components: Vec<Option<RuntimeComponent>> = vec![None; total_components];
    let mut generators: Vec<GeneratorIr> = Vec::new();
    let mut functions: Vec<FuncIr> = Vec::new();
    let mut top_levels: Vec<(String, JsExpr)> = Vec::new();
    let mut root_name: Option<String> = None;
    let mut module_irs: Vec<ModuleIr> = Vec::new();

    for id in &order {
        let m = &modules[id];
        let mut names: HashMap<String, usize> = own_name[id].clone();
        for decl in &m.program.decls {
            if let Decl::Import(import) = decl {
                // External packages resolve at runtime as builtins — no
                // canonical module, no export-surface check. (Same
                // resolve-error protocol as discovery: known ids link.)
                let target = match resolver.resolve(&import.path, &m.id) {
                    Ok(t) => t,
                    Err(_) if is_external_specifier(&import.path) => continue,
                    Err(e) => return Err(e),
                };
                // Resolve the raw specifier to its CANONICAL id (the key the
                // export surface uses), then map each imported binding to the
                // target's global component index.
                for (local, global) in resolve_import_bindings(&m.id, import, &target, &exports)? {
                    names.insert(local.clone(), global);
                }
            }
        }
        let (parts, gens, funcs, tls, default) = lower_module_parts(&m.program, &names)?;
        for (idx, comp) in parts {
            components[idx] = Some(comp);
        }
        generators.extend(gens);
        functions.extend(funcs);
        top_levels.extend(tls);
        if id == entry_id {
            // Entry root: `export default X` when present; otherwise the
            // entry's SOLE component (the `export function App()` shape real
            // React apps use — no default export required).
            let d = match default {
                Some(d) => Some(d),
                None => sole_entry_component(&m.program),
            };
            let d = d.ok_or_else(|| LinkError::NoDefault(entry_id.to_string()))?;
            root_name = Some(d);
        }
        let mut exps: Vec<(String, usize)> = exports[id]
            .iter()
            .filter(|(_, e)| e.kind == ExportKind::Component)
            .map(|(n, e)| (n.clone(), e.index))
            .collect();
        exps.sort();
        module_irs.push(ModuleIr {
            id: id.clone(),
            exports: exps,
        });
    }

    let root = root_name
        .and_then(|n| own_name.get(entry_id).and_then(|m| m.get(&n)).copied())
        .ok_or_else(|| LinkError::NoDefault(entry_id.to_string()))?;

    // 5. Assemble the merged artifact (dev/production strict-mode handling).
    let components: Vec<RuntimeComponent> = components
        .into_iter()
        .map(|c| c.ok_or_else(|| LinkError::Lower("component table gap".to_string())))
        .collect::<Result<_, _>>()?;

    let mut template = RuntimeTemplate {
        components,
        root,
        generators,
        functions,
        top_levels,
        modules: module_irs,
        manifest: RuntimeTemplate::new().manifest,
        strict_mode: false,
    };
    if !dev {
        for comp in &mut template.components {
            let b = std::mem::replace(
                &mut comp.body,
                ReactNode::Text(JsExpr::Lit(r2n_ast::lit::Literal::Null)),
            );
            comp.body = strip_strict(b);
        }
    } else {
        template.strict_mode = true;
    }
    Ok(template)
}

/// Parse a module source. Modules use `parse_module`, which (unlike the
/// single-file `parse`) does NOT require `export default` — only the entry
/// module must declare a root component (verified during linking).
fn parse_module(source: &str) -> Result<Program, LinkError> {
    let mut p = Parser::new(source)?;
    Ok(p.parse_module()?)
}

/// Sole-entry-component fallback: when the entry module has no
/// `export default` but declares exactly ONE component-shaped binding
/// (`component`, `class`, `export function`, or a `memo(function)` const),
/// that binding is the root (the `export function App()` shape real React
/// apps use). Zero or several → None (the caller raises NoDefault).
fn sole_entry_component(program: &Program) -> Option<String> {
    let mut found: Option<String> = None;
    let mut consider = |name: &str| {
        if found.is_none() {
            found = Some(name.to_string());
        } else {
            found = Some(String::new()); // marker: more than one
        }
    };
    for decl in &program.decls {
        match decl {
            Decl::Component(c) => consider(&c.name),
            Decl::Class(c) => consider(&c.name),
            Decl::ExportDecl(r2n_ast::program::ExportDecl::Function(f)) => consider(&f.name),
            Decl::ExportDecl(r2n_ast::program::ExportDecl::Const { name, value })
                if component_fn_of(value).is_some() =>
            {
                consider(name)
            }
            _ => {}
        }
    }
    match found {
        Some(n) if !n.is_empty() => Some(n),
        _ => None,
    }
}

/// The canonical ids a program statically imports (its `Decl::Import` paths),
/// in source order.
fn static_import_specifiers(program: &Program) -> Vec<String> {
    program
        .decls
        .iter()
        .filter_map(|d| match d {
            Decl::Import(i) => Some(i.path.clone()),
            _ => None,
        })
        .collect()
}

/// Visit every dynamic `import("specifier")` in a program's expressions and call
/// `f` with `&mut` access to each specifier (M2-T09). Covers component, class,
/// and generator bodies, nested expressions, JSX children, and attribute values
/// — anywhere a dynamic import can appear. Used both to discover dynamically
/// reachable modules and to canonicalize their specifiers to resolved module ids.
fn for_each_dyn_import_in_program(program: &mut Program, f: &mut impl FnMut(&mut String)) {
    for decl in &mut program.decls {
        match decl {
            Decl::Component(c) => {
                for stmt in &mut c.body {
                    for_each_dyn_import_in_stmt(stmt, f);
                }
            }
            Decl::Class(c) => {
                if let Some(state) = &mut c.state {
                    for_each_dyn_import(state, f);
                }
                for m in &mut c.methods {
                    for stmt in &mut m.body {
                        for_each_dyn_import_in_stmt(stmt, f);
                    }
                }
            }
            Decl::GeneratorFn(g) => {
                for stmt in &mut g.body {
                    for_each_dyn_import_in_stmt(stmt, f);
                }
            }
            Decl::FuncDecl(fd) => {
                for stmt in &mut fd.body {
                    for_each_dyn_import_in_stmt(stmt, f);
                }
            }
            Decl::TopLevel { value, .. } => for_each_dyn_import(value, f),
            Decl::ExportDecl(e) => match e {
                r2n_ast::program::ExportDecl::Function(fd) => {
                    for stmt in &mut fd.body {
                        for_each_dyn_import_in_stmt(stmt, f);
                    }
                }
                r2n_ast::program::ExportDecl::Const { value, .. } => for_each_dyn_import(value, f),
            },
            Decl::Import(_) | Decl::ExportDefault(_) | Decl::ExportNamed(_) => {}
        }
    }
}

fn for_each_dyn_import_in_stmt(stmt: &mut Stmt, f: &mut impl FnMut(&mut String)) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Const { value, .. } => for_each_dyn_import(value, f),
        Stmt::Destructure { value, .. } => for_each_dyn_import(value, f),
        Stmt::Return(e) => for_each_dyn_import(e, f),
        Stmt::Expr(e) => for_each_dyn_import(e, f),
        // Control-flow bodies can nest dynamic imports (e.g. lazy import in a
        // branch); walk them so discovery stays complete.
        Stmt::If { cond, then, else_ } => {
            for_each_dyn_import(cond, f);
            for s in then {
                for_each_dyn_import_in_stmt(s, f);
            }
            if let Some(e) = else_ {
                for s in e {
                    for_each_dyn_import_in_stmt(s, f);
                }
            }
        }
        Stmt::While { cond, body } => {
            for_each_dyn_import(cond, f);
            for s in body {
                for_each_dyn_import_in_stmt(s, f);
            }
        }
        Stmt::For {
            init,
            cond,
            update,
            body,
        } => {
            if let Some(i) = init {
                for_each_dyn_import_in_stmt(i, f);
            }
            if let Some(c) = cond {
                for_each_dyn_import(c, f);
            }
            if let Some(u) = update {
                for_each_dyn_import(u, f);
            }
            for s in body {
                for_each_dyn_import_in_stmt(s, f);
            }
        }
        Stmt::Switch { disc, cases } => {
            for_each_dyn_import(disc, f);
            for (_, body) in cases {
                for s in body {
                    for_each_dyn_import_in_stmt(s, f);
                }
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn for_each_dyn_import(expr: &mut Expr, f: &mut impl FnMut(&mut String)) {
    match expr {
        Expr::DynImport { specifier } => f(specifier),
        Expr::Member { base, .. } => for_each_dyn_import(base, f),
        Expr::Binary { left, right, .. } => {
            for_each_dyn_import(left, f);
            for_each_dyn_import(right, f);
        }
        Expr::Unary { expr, .. } => for_each_dyn_import(expr, f),
        Expr::Call { callee, args } => {
            for_each_dyn_import(callee, f);
            for a in args {
                match a {
                    r2n_ast::expr::CallArg::Expr(e) => for_each_dyn_import(e, f),
                    r2n_ast::expr::CallArg::Spread(e) => for_each_dyn_import(e, f),
                }
            }
        }
        Expr::New { callee, args } => {
            for_each_dyn_import(callee, f);
            for a in args {
                match a {
                    r2n_ast::expr::CallArg::Expr(e) => for_each_dyn_import(e, f),
                    r2n_ast::expr::CallArg::Spread(e) => for_each_dyn_import(e, f),
                }
            }
        }
        Expr::Assign { target, value } => {
            for_each_dyn_import(target, f);
            for_each_dyn_import(value, f);
        }
        Expr::Element(e) => {
            for c in &mut e.children {
                for_each_dyn_import(c, f);
            }
            for p in &mut e.props {
                if let Some(v) = &mut p.value {
                    for_each_dyn_import(v, f);
                }
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            for_each_dyn_import(cond, f);
            for_each_dyn_import(then, f);
            for_each_dyn_import(else_, f);
        }
        Expr::Array(items) => {
            for item in items {
                match item {
                    r2n_ast::expr::ArrayItem::Expr(e) => for_each_dyn_import(e, f),
                    r2n_ast::expr::ArrayItem::Spread(e) => for_each_dyn_import(e, f),
                }
            }
        }
        Expr::Object(items) => {
            for item in items {
                match item {
                    r2n_ast::expr::ObjectItem::Shorthand(_) => {}
                    r2n_ast::expr::ObjectItem::Prop(_, v) => for_each_dyn_import(v, f),
                    r2n_ast::expr::ObjectItem::Spread(e) => for_each_dyn_import(e, f),
                }
            }
        }
        Expr::Template { exprs, .. } => {
            for e in exprs {
                for_each_dyn_import(e, f);
            }
        }
        Expr::Update { target, .. } => for_each_dyn_import(target, f),
        Expr::CompoundAssign { target, value, .. } => {
            for_each_dyn_import(target, f);
            for_each_dyn_import(value, f);
        }
        Expr::Arrow { body, .. } => for_each_dyn_import(body, f),
        Expr::Function { body, .. } => {
            for s in body {
                for_each_dyn_import_in_stmt(s, f);
            }
        }
        Expr::Yield { value, .. } => {
            if let Some(v) = value {
                for_each_dyn_import(v, f);
            }
        }
        Expr::Await { value, .. } => for_each_dyn_import(value, f),
        Expr::Block(stmts) => {
            for s in stmts {
                for_each_dyn_import(s, f);
            }
        }
        Expr::Throw(v) => for_each_dyn_import(v, f),
        Expr::Return(v) => {
            if let Some(e) = v {
                for_each_dyn_import(e, f);
            }
        }
        Expr::While { cond, body } => {
            for_each_dyn_import(cond, f);
            for_each_dyn_import(body, f);
        }
        Expr::Break | Expr::Continue => {}
        Expr::Try {
            block,
            catch,
            finally,
            ..
        } => {
            for s in block {
                for_each_dyn_import(s, f);
            }
            if let Some(c) = catch {
                for s in c {
                    for_each_dyn_import(s, f);
                }
            }
            if let Some(fl) = finally {
                for s in fl {
                    for_each_dyn_import(s, f);
                }
            }
        }
        Expr::Literal(_) | Expr::Ident { .. } => {}
    }
}

/// Resolve an import declaration to `(local_binding, global_component_index)`
/// pairs for every bound name that refers to a COMPONENT/CLASS export. Names
/// bound to generator/function exports are not component positions and are
/// skipped so `<Name/>` never mis-lowers to a generator index (deferred to the
/// value-binding mechanism). `target` is the target module's canonical id;
/// returns an error for an unknown export.
fn resolve_import_bindings(
    from: &str,
    import: &Import,
    target: &str,
    exports: &HashMap<String, HashMap<String, Export>>,
) -> Result<Vec<(String, usize)>, LinkError> {
    let target_exports = exports
        .get(target)
        .ok_or_else(|| LinkError::UnknownExport {
            module: from.to_string(),
            exported: target.to_string(),
        })?;
    let mut out = Vec::new();
    if let Some(def) = &import.default_ {
        match target_exports.get("default") {
            Some(e) if e.kind == ExportKind::Component => out.push((def.clone(), e.index)),
            Some(_) => {} // generator default: deferred value binding
            None => {
                return Err(LinkError::UnknownExport {
                    module: from.to_string(),
                    exported: "default".to_string(),
                })
            }
        }
    }
    for (imported, local) in &import.named {
        match target_exports.get(imported) {
            Some(e) if e.kind == ExportKind::Component => out.push((local.clone(), e.index)),
            Some(_) => {} // generator value import: deferred
            None => {
                return Err(LinkError::UnknownExport {
                    module: from.to_string(),
                    exported: imported.clone(),
                })
            }
        }
    }
    // `import * as ns` binds a namespace OBJECT; dynamic member JSX use is
    // deferred. Nothing maps to a single component slot here.
    Ok(out)
}

/// Strip `<StrictMode>` wrappers from a React node tree (dev semantics out of
/// production). Closed-form twin of the lower.rs helper so the linker does not
/// need to import internal modules.
fn strip_strict(n: ReactNode) -> ReactNode {
    match n {
        ReactNode::StrictMode { children } => ReactNode::Fragment {
            key: None,
            children: children.into_iter().map(strip_strict).collect(),
        },
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
