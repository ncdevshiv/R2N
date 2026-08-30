//! The R2N runtime engine.
//!
//! Ties together evaluation (`eval`), hooks (`hooks`), and reconciliation
//! (`RenderedNode` -> `Patch[]`). Implements the core loop from the design:
//!   write state -> schedule dirty -> flush -> reconcile -> minimal Patch[]
//!
//! Key design points (matching the locked ADRs):
//! * Components are *inlined* into a flat host/text tree at render time; the
//!   rendered tree the renderer sees contains no component boundaries. The hook
//!   frames that back those components are tracked separately per component
//!   instance (by an *instance path*) in a `FrameStore`, so each logical
//!   component instance keeps its state across renders (the frame protocol,
//!   ADR-002/003).
//! * Every rendered node carries a `key`: its position for static children, the
//!   item key for list items, the branch name for conditionals. Reconciliation
//!   is therefore *keyed* (ADR-010): nodes match by `(type, key, path)`, which
//!   yields minimal, deterministic patches and correct list reordering.

use crate::eval::{eval, run_effect_body, Env, Host};
use crate::hooks::{EffectBody, HookFrame};
use crate::patch::{NodeId, Patch};
use crate::scheduler::Scheduler;
use crate::value::{RuntimeError, Value};
use r2n_ir::react::ReactNode;
use r2n_ir::runtime::RuntimeTemplate;
use std::collections::{BTreeMap, HashMap};

/// A snapshot of one component instance's render environment, saved so event
/// handlers can re-enter the runtime with the same scope (frame protocol).
#[derive(Debug, Clone)]
struct InstanceScope {
    env: Env,
}

/// A node in the *rendered* (already-inlined) tree. Contains only host elements
/// and text; component instances have been expanded by `render_node`. A list is
/// represented by a transparent fragment node (`tag == FRAGMENT`).
#[derive(Debug, Clone, PartialEq)]
pub enum RenderedNode {
    Host {
        tag: String,
        props: Vec<(String, Value)>,
        children: Vec<RenderedNode>,
        key: String,
    },
    Text {
        text: String,
        key: String,
    },
}

/// Transparent fragment tag: its children reconcile directly at the parent.
const FRAGMENT: &str = "\0frag";

/// A children splice: the pre-lowered nodes a parent passed to a component
/// instance (`<Card><b>hi</b></Card>`), the parent's env to evaluate them in
/// (composition is by reference — nodes keep closing over their original
/// scope), and the parent's instance path for hook-frame access.
#[derive(Clone)]
struct Splice {
    nodes: Vec<r2n_ir::react::ReactNode>,
    parent_env: Env,
    parent_inst: Vec<String>,
}

/// Active splices, keyed by the CHILD component's instance path. Populated
/// when a component call renders (its `children` prop), consumed when its
/// body hits the `ReactNode::Children` splice point.
type SpliceMap = HashMap<Vec<String>, Splice>;
#[derive(Debug, Default)]
pub struct FrameStore {
    frames: HashMap<Vec<String>, HookFrame>,
    /// Monotonic render-pass counter. Each `render_once` is one pass; a
    /// component's frame that skips a full pass was UNMOUNTED and must
    /// reset its hook state on remount (React unmount semantics).
    pass: u64,
}

impl FrameStore {
    fn get(&mut self, path: &[String]) -> &mut HookFrame {
        self.frames.entry(path.to_vec()).or_default()
    }

    /// Begin a new render pass; returns its number for `begin_render`.
    fn begin_pass(&mut self) -> u64 {
        self.pass += 1;
        self.pass
    }

    /// The current render-pass number (read before borrowing a frame).
    fn current_pass(&self) -> u64 {
        self.pass
    }

    /// Cleanups of frames NOT rendered in `pass`: they were unmounted this
    /// pass — drain and run their armed effect cleanups (React cleanup on
    /// unmount), disarming them so a later remount cannot run them again.
    fn take_unmounted_cleanups(&mut self, pass: u64) -> Vec<EffectBody> {
        let mut out = Vec::new();
        for frame in self.frames.values_mut() {
            if frame.last_pass() != Some(pass) {
                out.extend(frame.take_cleanups());
            }
        }
        out
    }

    /// Queue every dirty frame's instance path on the scheduler (deduped),
    /// clearing the per-frame dirty flags. Called after each render pass.
    fn schedule_dirty(&mut self, scheduler: &mut Scheduler) {
        let mut dirty_paths: Vec<Vec<String>> = Vec::new();
        for (path, frame) in self.frames.iter_mut() {
            if frame.take_dirty() {
                dirty_paths.push(path.clone());
            }
        }
        for path in dirty_paths {
            scheduler.schedule(path);
        }
    }
}

/// The runtime engine.
pub struct Runtime {
    template: RuntimeTemplate,
    frames: FrameStore,
    /// Render environments per component instance path, saved each flush so
    /// event handlers dispatched later run in the same scope (frame protocol).
    scopes: HashMap<Vec<String>, InstanceScope>,
    /// Stable node ids per rendered-node path (stable across renders).
    id_map: HashMap<Vec<String>, NodeId>,
    id_counter: u64,
    /// The previous rendered tree (for diffing).
    prev: Option<RenderedNode>,
    /// Log lines emitted by `console.log`.
    log: Vec<String>,
    /// Event handlers by node id, rebuilt on each flush: `node -> (event, handler)`.
    handlers: HashMap<NodeId, Vec<(String, Value)>>,
    /// FIFO render scheduler with per-instance dedup (M0.2-T04).
    scheduler: Scheduler,
}

struct LogHost<'a> {
    log: &'a mut Vec<String>,
}

impl<'a> Host for LogHost<'a> {
    fn log(&mut self, line: &str) {
        self.log.push(line.to_string());
    }
}

impl Runtime {
    pub fn new(template: RuntimeTemplate) -> Self {
        Self {
            template,
            frames: FrameStore::default(),
            scopes: HashMap::new(),
            id_map: HashMap::new(),
            id_counter: 1,
            prev: None,
            log: Vec::new(),
            handlers: HashMap::new(),
            scheduler: Scheduler::new(),
        }
    }

    pub fn template(&self) -> &RuntimeTemplate {
        &self.template
    }

    pub fn logs(&self) -> &[String] {
        &self.log
    }

    /// Render the root and return the patches that move the previous tree to
    /// the new one. Dirty frames (a setter ran) are enqueued on the FIFO
    /// scheduler — deduped per instance — and each queue entry triggers one
    /// re-render pass, drained in order. A bound prevents infinite update
    /// loops. After the loop the handler table reflects the final tree, so
    /// `dispatch` can fire events against it.
    pub fn flush(&mut self) -> Result<Vec<Patch>, RuntimeError> {
        let mut all = Vec::new();
        // First pass: the initial (or post-dispatch) render.
        self.render_once(&mut all)?;
        // Then drain the FIFO scheduler: any frames that went dirty during a
        // handler or a render are queued (deduped); each scheduled instance
        // re-renders once, in FIFO order.
        let mut guard = 0;
        while !self.scheduler.is_empty() {
            guard += 1;
            if guard > 1000 {
                self.scheduler.clear();
                return Err(RuntimeError::new("render loop exceeded 1000 iterations"));
            }
            // Pop the next scheduled instance; the render below re-evaluates
            // the whole tree top-down, which includes this instance.
            let _ = self.scheduler.pop_front();
            self.render_once(&mut all)?;
        }
        Ok(all)
    }

    /// One full render → diff → patch pass; queues any newly-dirty frames.
    fn render_once(&mut self, all: &mut Vec<Patch>) -> Result<(), RuntimeError> {
        let mut host = LogHost { log: &mut self.log };
        let mut scopes = std::mem::take(&mut self.scopes);
        self.frames.begin_pass();
        // Splices are rebuilt every pass: they ride the component-call props
        // (`Value::Children`), so a fresh render re-derives them naturally.
        let mut splices = SpliceMap::new();
        let tree = render_root(
            &self.template,
            &mut self.frames,
            &mut scopes,
            &mut splices,
            &mut host,
        )?;
        self.scopes = scopes;
        // Unmounted frames: their effects' cleanups run now (React cleanup
        // on unmount — resources release at unmount, not at a later remount).
        let unmount_cleanups = self
            .frames
            .take_unmounted_cleanups(self.frames.current_pass());
        if !unmount_cleanups.is_empty() {
            run_effects(&unmount_cleanups, &mut host, &self.template.components)?;
        }
        // Handlers are re-derived from the fresh tree each pass; start clean
        // so handlers on removed nodes disappear.
        let mut handlers = std::mem::take(&mut self.handlers);
        handlers.clear();
        let patches = diff(
            &mut self.id_map,
            &mut self.id_counter,
            self.prev.as_ref(),
            &tree,
            &mut handlers,
        );
        self.handlers = handlers;
        all.extend(patches);
        self.prev = Some(tree);
        // Frames that went dirty during this pass (setter calls in bindings,
        // effects, or the handler) get scheduled — deduped, FIFO.
        self.frames.schedule_dirty(&mut self.scheduler);
        Ok(())
    }

    /// Fire `event` (e.g. `"click"`, `"change"`) on `node` by running the
    /// matching handler from the last flushed tree, then flush until the tree
    /// is clean. This is the reactive loop's entry point: the handler body
    /// calls a state setter, the frame is marked dirty, and the flush below
    /// re-renders and emits the minimal patches.
    pub fn dispatch(&mut self, node: NodeId, event: &str) -> Result<Vec<Patch>, RuntimeError> {
        let (_, handler) = self
            .handlers
            .get(&node)
            .and_then(|evs| evs.iter().find(|(n, _)| n == event))
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("no '{event}' handler on node {node}")))?;
        let Value::Handler { inst_path, body } = handler else {
            return Err(RuntimeError::new("handler value has wrong shape"));
        };
        // Run the handler closure against its owning component's hook frame
        // and the env captured at render time (the frame protocol, ADR-002:
        // the event callback re-enters the runtime through the frame).
        // The handler body is a JsExpr::Closure — unwrap it and run the inner
        // expression against the instance's saved scope and frame. (Bare
        // `Closure` evaluation yields Null by design; handlers execute here.)
        let inner = match body.as_ref() {
            r2n_ir::js::JsExpr::Closure { body: b, .. } => b.clone(),
            other => Box::new(other.clone()),
        };
        let mut env = self
            .scopes
            .get(&inst_path)
            .map(|s| s.env.clone())
            .unwrap_or_default();
        let frame = self.frames.get(&inst_path);
        let mut effects: Vec<EffectBody> = Vec::new();
        let mut host = LogHost { log: &mut self.log };
        let result = eval(
            &inner,
            &mut env,
            frame,
            &mut host,
            &self.template.components,
            &mut effects,
        );
        result?;
        run_effects(&effects, &mut host, &self.template.components)?;
        self.flush()
    }
}

/// Render the root component into a `RenderedNode`. `FrameStore` and the
/// scope map are passed explicitly (rather than `&mut Runtime`) so recursive
/// `render_node` calls can borrow them and the template independently.
fn render_root(
    template: &RuntimeTemplate,
    frames: &mut FrameStore,
    scopes: &mut HashMap<Vec<String>, InstanceScope>,
    splices: &mut SpliceMap,
    host: &mut dyn Host,
) -> Result<RenderedNode, RuntimeError> {
    let root = template.root;
    let comp = template.components[root].clone();
    let path = vec!["root".to_string()];
    let pass = frames.current_pass();
    let frame = frames.get(&path);
    let unmount_cleanups = frame.begin_render(pass);
    let mut env = Env::new();
    let mut effects: Vec<EffectBody> = Vec::new();
    // Unmount cleanups run immediately: any state destroyed on unmount must
    // release its resources now (React cleanup-on-unmount ordering).
    run_effects(&unmount_cleanups, host, &template.components)?;
    for (name, expr) in &comp.bindings {
        let v = eval(
            expr,
            &mut env,
            frame,
            host,
            &template.components,
            &mut effects,
        )?;
        env.define(name, v);
    }
    scopes.insert(path.clone(), InstanceScope { env: env.clone() });
    let node_path = vec!["root".to_string()];
    let node = render_node(
        &comp.body,
        &path,
        &node_path,
        &mut env,
        frames,
        scopes,
        splices,
        template,
        host,
        &mut effects,
    )?;
    run_effects(&effects, host, &template.components)?;
    Ok(node)
}

#[allow(clippy::too_many_arguments)]
fn render_node(
    node: &ReactNode,
    inst_path: &[String],
    node_path: &[String],
    env: &mut Env,
    frames: &mut FrameStore,
    scopes: &mut HashMap<Vec<String>, InstanceScope>,
    splices: &mut SpliceMap,
    template: &RuntimeTemplate,
    host: &mut dyn Host,
    effects: &mut Vec<EffectBody>,
) -> Result<RenderedNode, RuntimeError> {
    Ok(match node {
        ReactNode::Host {
            tag,
            props,
            children,
        } => {
            let mut rprops = Vec::with_capacity(props.len());
            for (name, expr) in props {
                // Props evaluate in the owning component's frame (inst_path —
                // stable across this component's whole subtree), never in a
                // per-node orphan frame.
                let v = {
                    let frame = frames.get(inst_path);
                    eval(expr, env, frame, host, &template.components, effects)?
                };
                // An `on*` prop whose value is a closure is an event handler:
                // package it with this component instance's path so dispatch
                // later runs it against the right hook frame and scope.
                let v = if name.starts_with("on")
                    && matches!(expr, r2n_ir::js::JsExpr::Closure { .. })
                {
                    Value::Handler {
                        inst_path: inst_path.to_vec(),
                        body: Box::new(expr.clone()),
                    }
                } else {
                    v
                };
                rprops.push((name.clone(), v));
            }
            let mut rchildren: Vec<RenderedNode> = Vec::new();
            for (i, c) in children.iter().enumerate() {
                // Reconciliation identity (ADR-010): an author-provided `key`
                // is the child's identity; without one, identity is position
                // ('#i'). The key evaluates in the PARENT's scope — where the
                // element is written — exactly like React evaluates it at
                // element-creation time. A keyed child keeps its node id (and
                // its component instance, so hook state follows) across
                // reorders: reconciliation emits Move, never Remove+Create.
                let key_seg = match static_key_expr(c) {
                    Some(expr) => {
                        let v = {
                            let frame = frames.get(inst_path);
                            eval(expr, env, frame, host, &template.components, effects)?
                        };
                        format!("k:{}", v.display())
                    }
                    None => format!("#{i}"),
                };
                let child_node = child_path(node_path, i, &key_seg);
                let rn = render_node(
                    c,
                    inst_path,
                    &child_node,
                    env,
                    frames,
                    scopes,
                    splices,
                    template,
                    host,
                    effects,
                )?;
                if let RenderedNode::Host { tag, .. } = &rn {
                    if tag == FRAGMENT {
                        // Fragments are TRANSPARENT: splice their children
                        // into this parent's child list at this position.
                        // (Splicing at render time keeps diff_children indices
                        // correct — a list fragment followed by a sibling gets
                        // flat sibling positions, so Move/Create indices line
                        // up on every renderer.)
                        // List items KEEP their keyed identity (stable across
                        // renders — the whole point of keys: a moved item is
                        // the SAME node, not a new one).
                        if let RenderedNode::Host { children: fc, .. } = rn {
                            rchildren.extend(fc);
                        }
                        continue;
                    }
                }
                // A keyed child under a flipped conditional is STILL the same
                // child (React semantics): its author key overrides the
                // positional sibling identity, so a branch flip preserves the
                // node id and the diff is a Move, not Remove+Create.
                let key_seg = match &rn {
                    other if static_key_expr(c).is_some() => other.key().to_string(),
                    _ => key_seg,
                };
                // Static (non-list) siblings: identity is the key when the
                // author provided one, else the position ('#i').
                let mut rn = rn;
                set_key(&mut rn, &key_seg);
                rchildren.push(rn);
            }
            RenderedNode::Host {
                tag: tag.clone(),
                props: rprops,
                children: rchildren,
                key: "h".to_string(),
            }
        }
        ReactNode::Text(expr) => {
            let v = {
                let frame = frames.get(inst_path);
                eval(expr, env, frame, host, &template.components, effects)?
            };
            // React children semantics: `true`, `false`, `null`, and
            // `undefined` render NOTHING (so `{flag && "text"}` can ride the
            // Text path — a falsy short-circuit result disappears). Numbers
            // (including 0) and strings render — the classic `0` footgun is
            // React parity, deliberately preserved.
            if matches!(v, Value::Null | Value::Bool(_)) {
                return Ok(RenderedNode::Host {
                    tag: FRAGMENT.to_string(),
                    props: Vec::new(),
                    children: Vec::new(),
                    key: "t".to_string(),
                });
            }
            RenderedNode::Text {
                text: v.display(),
                key: "t".to_string(),
            }
        }
        ReactNode::Component { component, props } => {
            let comp = template.components[component.index()].clone();
            // This component instance gets its own path: parent's instance
            // path + the node position (unique per sibling) + the component
            // name — so two <Counter/> siblings never share a frame.
            let inst = child_inst_path(
                inst_path,
                node_path.len(), // depth-stable position
                &format!(
                    "{}{}",
                    comp.name,
                    node_path.last().map(|s| s.as_str()).unwrap_or("")
                ),
            );
            // Props are evaluated in the PARENT's scope and frame.
            let mut prop_vals: Vec<(String, Value)> = Vec::with_capacity(props.len());
            let mut children_nodes: Option<Vec<r2n_ir::react::ReactNode>> = None;
            for (name, expr) in props {
                let v = {
                    let frame = frames.get(inst_path);
                    eval(expr, env, frame, host, &template.components, effects)?
                };
                // The `children` prop carries pre-lowered nodes to splice at
                // the child's `ReactNode::Children` point. Keep the nodes
                // AND remember the parent scope they close over.
                if name == "children" {
                    if let Value::Children(nodes) = &v {
                        children_nodes = Some(nodes.clone());
                    }
                }
                prop_vals.push((name.clone(), v));
            }
            // Record the splice for this instance (the child body's
            // `ReactNode::Children` consumes it below). If this instance
            // renders again, the splice is refreshed here first.
            if let Some(nodes) = children_nodes {
                splices.insert(
                    inst.clone(),
                    Splice {
                        nodes,
                        parent_env: env.clone(),
                        parent_inst: inst_path.to_vec(),
                    },
                );
            } else {
                // No children passed: an existing splice from a previous
                // render is stale — the prop disappeared; render nothing.
                splices.remove(&inst);
            }
            // The instance's own hook frame + env (the frame protocol).
            let cenv = {
                let pass = frames.current_pass();
                let cframe = frames.get(&inst);
                let unmount_cleanups = cframe.begin_render(pass);
                if !unmount_cleanups.is_empty() {
                    run_effects(&unmount_cleanups, host, &template.components)?;
                }
                let mut cenv = Env::new();
                for (p, v) in comp
                    .params
                    .iter()
                    .zip(prop_vals.into_iter().map(|(_, v)| v))
                {
                    cenv.define(p, v);
                }
                let mut ceffects: Vec<EffectBody> = Vec::new();
                for (name, expr) in &comp.bindings {
                    let v = {
                        let cf = frames.get(&inst);
                        eval(
                            expr,
                            &mut cenv,
                            cf,
                            host,
                            &template.components,
                            &mut ceffects,
                        )?
                    };
                    cenv.define(name, v);
                }
                run_effects(&ceffects, host, &template.components)?;
                cenv
            };
            // Save this instance's scope so handlers dispatched later run in
            // it (the frame protocol's re-entry channel).
            scopes.insert(inst.clone(), InstanceScope { env: cenv.clone() });
            // Render the body in this instance's scope; node identity continues
            // from the same position, instance path switches to the child's.
            let mut body_env = cenv;
            render_node(
                &comp.body,
                &inst,
                node_path,
                &mut body_env,
                frames,
                scopes,
                splices,
                template,
                host,
                effects,
            )?
        }
        ReactNode::If { cond, then, else_ } => {
            let c = {
                let frame = frames.get(inst_path);
                eval(cond, env, frame, host, &template.components, effects)?
            };
            let branch = if c.is_truthy() { then } else { else_ };
            let branch_key = if c.is_truthy() { "then" } else { "else" };
            // An author-provided key on the branch child is its identity
            // (and must SURVIVE the branch flip — the same keyed child
            // rendered by the other branch is still the same child). The
            // positional then/else marker is only for unkeyed children.
            // The keyed child's path is node_path AS-IS: the parent's
            // children loop already appended the key segment (identity is
            // position-free), so appending here would double it and break
            // id_map matching between render and diff.
            let (child_node, key_seg) = match static_key_expr(branch) {
                Some(expr) => {
                    let v = {
                        let frame = frames.get(inst_path);
                        eval(expr, env, frame, host, &template.components, effects)?
                    };
                    (node_path.to_vec(), format!("k:{}", v.display()))
                }
                None => (child_path(node_path, 0, branch_key), branch_key.to_string()),
            };
            let mut rn = render_node(
                branch,
                inst_path,
                &child_node,
                env,
                frames,
                scopes,
                splices,
                template,
                host,
                effects,
            )?;
            set_key(&mut rn, &key_seg);
            rn
        }
        ReactNode::List {
            items,
            key_expr,
            item,
        } => {
            let arr = {
                let frame = frames.get(inst_path);
                match eval(items, env, frame, host, &template.components, effects)? {
                    Value::Array(a) => a,
                    other => {
                        return Err(RuntimeError::new(format!(
                            "list items must be an array, got {other}"
                        )))
                    }
                }
            };
            let mut out = Vec::with_capacity(arr.len());
            for (i, elem) in arr.into_iter().enumerate() {
                env.push_scope();
                env.define("$item", elem);
                let key = {
                    let frame = frames.get(inst_path);
                    let k = eval(key_expr, env, frame, host, &template.components, effects)?;
                    k.display()
                };
                let child_node = child_path(node_path, i, &key);
                let mut rn = render_node(
                    item,
                    inst_path,
                    &child_node,
                    env,
                    frames,
                    scopes,
                    splices,
                    template,
                    host,
                    effects,
                )?;
                set_key(&mut rn, &key);
                out.push(rn);
                env.pop_scope();
            }
            RenderedNode::Host {
                tag: FRAGMENT.to_string(),
                props: Vec::new(),
                children: out,
                key: "frag".to_string(),
            }
        }
        ReactNode::Fragment { children, .. } => {
            // A `<>...</>` group: no host element of its own. Rendered as the
            // transparent FRAGMENT host — the parent's children loop splices
            // its children into place, so siblings flow around it and the
            // children keep their own (possibly keyed) identity. The `key` on
            // the fragment matters only when it is a LIST item (the List arm
            // overrides the key below); any other position ignores it.
            //
            // Child keys are scoped by the fragment's own path segment
            // (`<parent-seg>:<i>`): after splicing, the fragment's children
            // are SIBLINGS of the parent's other children — bare `#i` keys
            // would collide with the parent's positional keys and corrupt
            // reconciliation (stale nodes survive branch flips).
            let frag_seg = node_path
                .last()
                .cloned()
                .unwrap_or_else(|| "frag".to_string());
            let mut out = Vec::with_capacity(children.len());
            for (i, c) in children.iter().enumerate() {
                let key = format!("{frag_seg}:{i}");
                let child_node = child_path(node_path, i, &key);
                let mut rn = render_node(
                    c,
                    inst_path,
                    &child_node,
                    env,
                    frames,
                    scopes,
                    splices,
                    template,
                    host,
                    effects,
                )?;
                set_key(&mut rn, &key);
                out.push(rn);
            }
            RenderedNode::Host {
                tag: FRAGMENT.to_string(),
                props: Vec::new(),
                children: out,
                key: "frag".to_string(),
            }
        }
        ReactNode::Children => {
            // The parent's children splice point. Render each stored node in
            // the PARENT's env (against the parent's hook frame) — composition
            // by reference: the nodes still close over their original scope,
            // so a `{n}` inside `<Card>{n}</Card>` reads the parent's `n`.
            // A component that receives no children renders nothing here.
            let mut out = Vec::new();
            if let Some(splice) = splices.get(inst_path).cloned() {
                let Splice {
                    nodes,
                    parent_env,
                    parent_inst,
                } = splice;
                let mut penv = parent_env.clone();
                let pinst = parent_inst.clone();
                for (i, node) in nodes.iter().enumerate() {
                    let child_node = child_path(node_path, i, &format!("^{i}"));
                    let mut rn = render_node(
                        node,
                        &pinst,
                        &child_node,
                        &mut penv,
                        frames,
                        scopes,
                        splices,
                        template,
                        host,
                        effects,
                    )?;
                    set_key(&mut rn, &format!("^{i}"));
                    out.push(rn);
                }
            }
            RenderedNode::Host {
                tag: FRAGMENT.to_string(),
                props: Vec::new(),
                children: out,
                key: "cfrag".to_string(),
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn diff(
    id_map: &mut HashMap<Vec<String>, NodeId>,
    id_counter: &mut u64,
    old: Option<&RenderedNode>,
    new: &RenderedNode,
    handlers: &mut HashMap<NodeId, Vec<(String, Value)>>,
) -> Vec<Patch> {
    let mut patches = Vec::new();
    let mut next_id_map = HashMap::new();
    diff_node(
        id_map,
        id_counter,
        old,
        new,
        &[],
        None,
        0,
        &mut patches,
        &mut next_id_map,
        handlers,
    );
    *id_map = next_id_map;
    patches
}

#[allow(clippy::too_many_arguments)]
fn diff_node(
    id_map: &mut HashMap<Vec<String>, NodeId>,
    id_counter: &mut u64,
    old: Option<&RenderedNode>,
    new: &RenderedNode,
    path: &[String],
    parent: Option<NodeId>,
    index: usize,
    patches: &mut Vec<Patch>,
    next_id_map: &mut HashMap<Vec<String>, NodeId>,
    handlers: &mut HashMap<NodeId, Vec<(String, Value)>>,
) {
    let id = *id_map.get(path).unwrap_or(&{
        let id = NodeId(*id_counter);
        *id_counter += 1;
        id
    });
    next_id_map.insert(path.to_vec(), id);

    match new {
        RenderedNode::Host {
            tag,
            props,
            children,
            ..
        } => {
            // Register this node's event handlers (on* props) for dispatch.
            let evs: Vec<(String, Value)> = props
                .iter()
                .filter(|(n, _)| n.starts_with("on"))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            if !evs.is_empty() {
                handlers.insert(id, evs);
            } else {
                handlers.remove(&id);
            }
            let same_tag = matches!(old, Some(RenderedNode::Host { tag: ot, .. }) if ot == tag);
            let is_frag = tag == FRAGMENT;
            if !same_tag {
                if let Some(RenderedNode::Host { children: oc, .. }) = old {
                    for (i, c) in oc.iter().enumerate() {
                        let cp = child_path(path, i, c.key());
                        remove_recursive(id_map, c, &cp, patches);
                    }
                }
                if !is_frag {
                    patches.push(Patch::Create {
                        id,
                        parent,
                        index,
                        tag: tag.clone(),
                    });
                    for (name, value) in props {
                        // `key` is reconciliation metadata, not a DOM prop.
                        if name == "key" {
                            continue;
                        }
                        patches.push(Patch::SetProp {
                            id,
                            name: name.clone(),
                            value: value.clone(),
                        });
                    }
                }
                let flats = flat_positions(children);
                for (i, c) in children.iter().enumerate() {
                    let cp = child_path(path, i, c.key());
                    let (child_parent, child_index) = if is_frag {
                        (parent, index + flats[i])
                    } else {
                        (Some(id), flats[i])
                    };
                    diff_node(
                        id_map,
                        id_counter,
                        None,
                        c,
                        &cp,
                        child_parent,
                        child_index,
                        patches,
                        next_id_map,
                        handlers,
                    );
                }
            } else if is_frag {
                diff_children(
                    id_map,
                    id_counter,
                    old,
                    children,
                    path,
                    parent,
                    patches,
                    next_id_map,
                    handlers,
                );
            } else {
                if let Some(RenderedNode::Host { props: op, .. }) = old {
                    diff_props(id, op, props, patches);
                }
                diff_children(
                    id_map,
                    id_counter,
                    old,
                    children,
                    path,
                    Some(id),
                    patches,
                    next_id_map,
                    handlers,
                );
            }
        }
        RenderedNode::Text { text, .. } => {
            if let Some(RenderedNode::Text { text: ot, .. }) = old {
                if ot != text {
                    patches.push(Patch::SetText {
                        id,
                        text: text.clone(),
                    });
                }
            } else {
                patches.push(Patch::CreateText {
                    id,
                    parent,
                    index,
                    text: text.clone(),
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn diff_children(
    id_map: &mut HashMap<Vec<String>, NodeId>,
    id_counter: &mut u64,
    old: Option<&RenderedNode>,
    new_children: &[RenderedNode],
    path: &[String],
    parent_id: Option<NodeId>,
    patches: &mut Vec<Patch>,
    next_id_map: &mut HashMap<Vec<String>, NodeId>,
    handlers: &mut HashMap<NodeId, Vec<(String, Value)>>,
) {
    let old_children: Vec<&RenderedNode> = match old {
        Some(RenderedNode::Host { children, .. }) => children.iter().collect(),
        _ => Vec::new(),
    };
    let old_by_key: HashMap<String, (usize, &RenderedNode)> = old_children
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key().to_string(), (i, *c)))
        .collect();
    let old_order: Vec<String> = old_children.iter().map(|c| c.key().to_string()).collect();

    let flats = flat_positions(new_children);
    for (i, c) in new_children.iter().enumerate() {
        let key = c.key().to_string();
        let cp = child_path(path, i, &key);
        let old_node = old_by_key.get(&key).map(|(_, n)| *n);
        diff_node(
            id_map,
            id_counter,
            old_node,
            c,
            &cp,
            parent_id,
            flats[i],
            patches,
            next_id_map,
            handlers,
        );
    }

    // Removals.
    for (key, (_, old_node)) in &old_by_key {
        if !new_children.iter().any(|c| c.key() == key) {
            let idx = old_order.iter().position(|k| k == key).unwrap_or(0);
            let cp = child_path(path, idx, key);
            remove_recursive(id_map, old_node, &cp, patches);
        }
    }

    // Moves: a surviving child whose RELATIVE order changed needs a Move
    // (removals alone shift siblings — that must not trigger Moves). The
    // Move's index is the child's ABSOLUTE position in the new list:
    // survivor-relative indices diverge from absolute ones when new nodes
    // are created interleaved among moved survivors.
    let surviving_old: Vec<&String> = old_order
        .iter()
        .filter(|k| new_children.iter().any(|c| c.key() == **k))
        .collect();
    let surviving_new: Vec<String> = new_children
        .iter()
        .map(|c| c.key().to_string())
        .filter(|k| old_order.contains(k))
        .collect();
    let new_pos: HashMap<&str, usize> = new_children
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key(), i))
        .collect();
    let new_flats = flat_positions(new_children);
    for (rel_new_i, key) in surviving_new.iter().enumerate() {
        let rel_old_i = surviving_old
            .iter()
            .position(|k| *k == key)
            .unwrap_or(rel_new_i);
        if rel_old_i != rel_new_i {
            let list_pos = new_pos.get(key.as_str()).copied().unwrap_or(rel_new_i);
            // The Move targets the child's flat renderer position (fragment
            // siblings occupy multiple slots). Fragment children have no
            // renderer node of their own — the id lookup misses and the
            // move is skipped for them (their children move individually).
            let abs_new_i = new_flats.get(list_pos).copied().unwrap_or(list_pos);
            if let Some(id) = next_id_map.get(&child_path(path, abs_new_i, key)) {
                patches.push(Patch::Move {
                    id: *id,
                    parent: parent_id,
                    index: abs_new_i,
                });
            }
        }
    }
}

fn remove_recursive(
    id_map: &HashMap<Vec<String>, NodeId>,
    node: &RenderedNode,
    path: &[String],
    patches: &mut Vec<Patch>,
) {
    if let Some(id) = id_map.get(path).copied() {
        patches.push(Patch::Remove { id });
    }
    if let RenderedNode::Host { children, .. } = node {
        for (i, c) in children.iter().enumerate() {
            let cp = child_path(path, i, c.key());
            remove_recursive(id_map, c, &cp, patches);
        }
    }
}

impl RenderedNode {
    fn key(&self) -> &str {
        match self {
            RenderedNode::Host { key, .. } => key,
            RenderedNode::Text { key, .. } => key,
        }
    }
}

/// The `key` prop expression of a static child (host element, component
/// call, or — looked through — a conditional's keyed branch), if the author
/// provided one. List items are excluded: their keys are handled by the
/// `List` arm (which owns item identity). The runtime may use this to key
/// reconciliation BEFORE rendering the child (identity must be known when
/// the child's node path is built, not after) — for an `If`, the key of the
/// keyed branch child is the identity of whichever branch renders, so the
/// same keyed child survives a branch flip.
fn static_key_expr(node: &ReactNode) -> Option<&r2n_ir::js::JsExpr> {
    match node {
        ReactNode::Host { props, .. } | ReactNode::Component { props, .. } => {
            props.iter().find(|(n, _)| n == "key").map(|(_, e)| e)
        }
        ReactNode::If { then, else_, .. } => {
            static_key_expr(then).or_else(|| static_key_expr(else_))
        }
        _ => None,
    }
}

/// Flat renderer positions for a child list. Fragments are TRANSPARENT:
/// they never create a renderer node — each contributes `children.len()`
/// flat slots, everything else contributes exactly one. When fragment
/// siblings exist (e.g. `.map` items that are `<>` groups), a later
/// sibling's flat index is NOT its list position; Create/Move indices must
/// use these flat positions or the rendered child order breaks.
fn flat_positions(children: &[RenderedNode]) -> Vec<usize> {
    let mut out = Vec::with_capacity(children.len());
    let mut flat = 0usize;
    for c in children {
        out.push(flat);
        flat += match c {
            RenderedNode::Host {
                tag, children: fc, ..
            } if tag == FRAGMENT => fc.len(),
            _ => 1,
        };
    }
    out
}

fn child_path(parent: &[String], index: usize, key: &str) -> Vec<String> {
    // * An author-provided KEY (list items) is identity: the node keeps its
    //   id across renders even when it moves — position is not encoded.
    // * A static sibling (no key) is identified by POSITION: `#i`. (Two
    //   <Counter/> siblings are different because they sit at different
    //   positions; swapping them swaps identity.)
    let seg = if let Some(pos) = key.strip_prefix('#') {
        format!("#{pos}")
    } else {
        key.to_string()
    };
    let _ = index;
    let mut p = parent.to_vec();
    p.push(seg);
    p
}

fn diff_props(
    id: NodeId,
    old: &[(String, Value)],
    new: &[(String, Value)],
    patches: &mut Vec<Patch>,
) {
    // `key` is reconciliation metadata consumed by the runtime; it never
    // crosses into the renderer as a prop (React strips it too).
    let old_map: BTreeMap<&str, &Value> = old
        .iter()
        .filter(|(k, _)| k != "key")
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    let new_map: BTreeMap<&str, &Value> = new
        .iter()
        .filter(|(k, _)| k != "key")
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    for (k, v) in &new_map {
        match old_map.get(k) {
            Some(ov) if ov == v => {}
            _ => patches.push(Patch::SetProp {
                id,
                name: (*k).to_string(),
                value: (*v).clone(),
            }),
        }
    }
    for k in old_map.keys() {
        if !new_map.contains_key(k) {
            patches.push(Patch::SetProp {
                id,
                name: (*k).to_string(),
                value: Value::Null,
            });
        }
    }
}

fn run_effects(
    effects: &[EffectBody],
    host: &mut dyn Host,
    components: &[r2n_ir::runtime::RuntimeComponent],
) -> Result<(), RuntimeError> {
    for e in effects {
        let mut env = e.env.clone();
        run_effect_body(&e.body, &mut env, host, components)?;
    }
    Ok(())
}

fn child_inst_path(parent: &[String], index: usize, kind: &str) -> Vec<String> {
    let mut p = parent.to_vec();
    p.push(format!("{kind}{index}"));
    p
}

fn set_key(node: &mut RenderedNode, key: &str) {
    match node {
        RenderedNode::Host { key: k, .. } => *k = key.to_string(),
        RenderedNode::Text { key: k, .. } => *k = key.to_string(),
    }
}
