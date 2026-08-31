//! Hooks — `useState` / `useEffect` with React's per-instance, order-based
//! contract, implemented via the "frame protocol" (ADR-002/ADR-003).
//!
//! Each render of a component runs inside a `HookFrame` that owns a `Vec` of
//! hook slots. Hooks read/write slots by their call index (the canonical
//! React rule: hooks must be called in the same order every render). Calling a
//! hook dispatches into the frame, which is why JS IR "calls back into React":
//! the `useState` identifier in JS IR resolves to a builtin that mutates the
//! current frame's slot. This is the ADR-002 interlink made operational.

use crate::value::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// Globally-unique id source for `useId` (like React's :rN: ids).
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Per-instance hook storage. `slots` are append-only during a render up to the
/// previous render's length; `effects` collects `useEffect` calls made this
/// render (the runtime flushes them after commit).
#[derive(Debug, Clone, Default)]
pub struct HookFrame {
    slots: Vec<HookSlot>,
    next_index: usize,
    effects: Vec<Effect>,
    /// Dirty flag set when a state setter is called; the scheduler re-renders.
    dirty: bool,
    /// The last render pass this frame took part in (`None` = never rendered).
    /// A frame absent for a full pass was unmounted — remounting resets its
    /// state (React: unmount destroys component state).
    last_pass: Option<u64>,
    /// The component instance path this frame belongs to (stamped by
    /// `FrameStore::get`). Needed to build `Value::Handler` values
    /// (useCallback) that must resolve the owning component's scope.
    path: Option<Vec<String>>,
    /// Renders this frame has performed (for class lifecycle: didMount on
    /// the first, didUpdate afterwards).
    render_count: u64,
    /// Monotonic source of useCallback identity numbers.
    next_cb_ident: u64,
}

#[derive(Debug, Clone)]
enum HookSlot {
    State {
        value: Value,
    },
    /// `useReducer`: the reducer's params/body and the current state.
    /// The dispatcher evaluates `reducer(state, action)` on each dispatch.
    Reducer {
        params: Vec<String>,
        body: r2n_ir::js::JsExpr,
        state: Value,
    },
    /// `useEffect`: deps of the last run (or None = run every render) and
    /// the currently-armed cleanup (body + captured env). The old cleanup
    /// runs immediately before the effect re-runs (deps changed) and on
    /// unmount — React cleanup semantics.
    Effect {
        deps: Option<Vec<Value>>,
        cleanup: Option<EffectBody>,
    },
    /// `useRef`: the current value held by the ref box (the slot index is
    /// the identity across renders).
    RefValue {
        value: Value,
    },
    /// `useId`: a globally-unique id created once for this instance's
    /// lifetime (a remounted frame starts fresh — a new id, like React).
    Id {
        value: Value,
    },
    /// `useMemo`: the deps of the last computation and its cached value.
    /// The compute runs only when the deps changed (or first render).
    Memo {
        deps: Option<Vec<Value>>,
        value: Value,
    },
    /// `useCallback`: the deps of the last registration and the cached
    /// function value (a `Value::Handler`). Identity is stable across
    /// renders while deps are unchanged — so a callback in an effect-deps
    /// array does not re-trigger its effect, and an `onClick={f}` prop
    /// keeps the same handler.
    Callback {
        deps: Option<Vec<Value>>,
        value: Value,
    },
}

/// A `useEffect` registration made during a render.
#[derive(Debug, Clone)]
pub struct Effect {
    pub deps: Option<Vec<Value>>,
    /// The previous deps, for change detection on the next render.
    pub prev_deps: Option<Vec<Value>>,
}

/// A user effect body captured for deferred execution after commit.
#[derive(Debug, Clone)]
pub struct EffectBody {
    pub body: r2n_ir::js::JsExpr,
    pub env: crate::eval::Env,
    /// `useLayoutEffect` (true) drains synchronously during the render walk
    /// — before the diff produces the patch stream (pre-commit). Regular
    /// `useEffect` (false) drains after the diff (post-commit).
    pub layout: bool,
    /// The owning component's instance path: hook handles (refs, setters)
    /// referenced by the body live in ITS frame, not a throwaway one.
    pub frame_path: Option<Vec<String>>,
}

impl HookFrame {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    /// `useState(initial)`: returns the current value and a setter.
    pub fn use_state(&mut self, initial: Value) -> (Value, Setter) {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            self.slots.push(HookSlot::State {
                value: initial.clone(),
            });
        }
        let value = match self.slots.get(idx) {
            Some(HookSlot::State { value, .. }) => value.clone(),
            _ => initial,
        };
        (value, Setter { frame_index: idx })
    }

    /// Apply a queued state update from a setter. `updater` is `None` for a
    /// direct value, or `Some(f)` for a functional update `(prev) => next`.
    pub fn apply_setter(&mut self, s: &Setter, new_value: Value) {
        if let Some(HookSlot::State { value }) = self.slots.get_mut(s.frame_index) {
            if *value != new_value {
                *value = new_value;
                self.dirty = true;
            }
        }
    }

    /// `useReducer(reducer, initial)`: stores the reducer closure (params +
    /// body — never a function pointer) and the current state. Returns
    /// `(state, dispatch)`. The action is a plain value in this subset
    /// (React action objects are M2 object-graph work); dispatch evaluates
    /// `reducer(state, action)` in a fresh env bound only to its params.
    pub fn use_reducer(
        &mut self,
        params: Vec<String>,
        body: r2n_ir::js::JsExpr,
        initial: Value,
    ) -> (Value, Value) {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            self.slots.push(HookSlot::Reducer {
                params,
                body,
                state: initial.clone(),
            });
        }
        let state = match self.slots.get(idx) {
            Some(HookSlot::Reducer { state, .. }) => state.clone(),
            _ => initial,
        };
        (state, Value::Dispatcher { slot: idx })
    }

    /// The component instance path this frame belongs to (None for
    /// throwaway frames, e.g. effect bodies).
    pub fn path(&self) -> Option<&[String]> {
        self.path.as_deref()
    }

    /// Stamp the frame with its instance path (FrameStore::get).
    pub fn set_path(&mut self, path: Vec<String>) {
        self.path = Some(path);
    }

    /// A fresh identity number for a useCallback registration.
    /// Is this the FIRST render of the frame (mount)? Used by class
    /// componentDidMount vs componentDidUpdate.
    pub fn is_first_render(&self) -> bool {
        self.render_count <= 1
    }

    pub fn next_callback_ident(&mut self) -> u64 {
        self.next_cb_ident += 1;
        self.next_cb_ident
    }

    /// The pass this frame last rendered in (None = never).
    pub fn last_pass(&self) -> Option<u64> {
        self.last_pass
    }

    /// Take every ARMED cleanup (used by the runtime for unmounted frames:
    /// the frame was absent for this render pass — its effects' cleanups
    /// run now, once, and are disarmed so remount cannot run them again).
    pub fn take_cleanups(&mut self) -> Vec<EffectBody> {
        let mut out = Vec::new();
        for slot in &mut self.slots {
            if let HookSlot::Effect { cleanup, .. } = slot {
                if let Some(c) = cleanup.take() {
                    out.push(c);
                }
            }
        }
        out
    }

    /// Read the reducer and its current state for dispatch evaluation.
    pub fn reducer_state(&self, idx: usize) -> Option<(Vec<String>, r2n_ir::js::JsExpr, Value)> {
        match self.slots.get(idx) {
            Some(HookSlot::Reducer {
                params,
                body,
                state,
            }) => Some((params.clone(), body.clone(), state.clone())),
            _ => None,
        }
    }

    /// `useResource(key)`: `(Value::Pending, resolve_setter)` — a
    /// suspension source. Reads are Pending until the setter fires.
    pub fn use_pending(&mut self, key: Value) -> (Value, Value) {
        let idx = self.next_index;
        self.next_index += 1;
        let cur = if idx >= self.slots.len() {
            self.slots.push(HookSlot::State {
                value: Value::Pending,
            });
            Value::Pending
        } else {
            match &self.slots[idx] {
                HookSlot::State { value } => value.clone(),
                _ => Value::Pending,
            }
        };
        let _ = key;
        (
            cur,
            Value::Setter(crate::hooks::Setter { frame_index: idx }),
        )
    }

    /// `useId()`: the same `Value` for every render of this instance; a
    /// fresh one after unmount/remount (slot cleared on reset). The id is
    /// globally unique and strings like `:r1:` (React's typical shape).
    pub fn use_id(&mut self) -> Value {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            static NEXT: u64 = 0; // replaced below with atomic
            let _ = NEXT;
            let v = Value::from_str_utf8(&format!(":r{}:", crate::hooks::next_id()));
            self.slots.push(HookSlot::Id { value: v.clone() });
            return v;
        }
        match self.slots.get(idx) {
            Some(HookSlot::Id { value }) => value.clone(),
            _ => {
                let v = Value::from_str_utf8(&format!(":r{}:", crate::hooks::next_id()));
                self.slots[idx] = HookSlot::Id { value: v.clone() };
                v
            }
        }
    }

    /// `useRef(initial)`: returns a ref handle (same slot identity across
    /// renders). `.current` writes persist without re-render.
    pub fn use_ref(&mut self, initial: Value) -> Value {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            self.slots.push(HookSlot::RefValue {
                value: initial.clone(),
            });
        }
        Value::Ref { slot: idx }
    }

    /// Read a ref's `.current`.
    pub fn read_ref(&self, slot: usize) -> Option<Value> {
        match self.slots.get(slot) {
            Some(HookSlot::RefValue { value }) => Some(value.clone()),
            _ => None,
        }
    }

    /// Write a ref's `.current`.
    pub fn write_ref(&mut self, slot: usize, value: Value) {
        if let Some(HookSlot::RefValue { value: v }) = self.slots.get_mut(slot) {
            *v = value;
        }
    }

    /// `useMemo(deps)`: returns `Some(cached)` when the deps are unchanged
    /// (reuse the cached value) or `None` when the caller must compute and
    /// then `record_memo` it (first render or deps changed).
    pub fn use_memo(&mut self, deps: Option<Vec<Value>>) -> Option<Value> {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            self.slots.push(HookSlot::Memo {
                deps,
                value: Value::Null,
            });
            return None;
        }
        let cached = match &self.slots[idx] {
            HookSlot::Memo { deps: d, value } => {
                if deps_eq(d, &deps) {
                    Some(value.clone())
                } else {
                    None
                }
            }
            _ => None,
        };
        match cached {
            Some(v) => Some(v),
            None => {
                // Record the new deps NOW: the value is computed by the
                // caller (record_memo fills it in), but the deps must be
                // current or the next render would see them as changed
                // again and recompute wrongly.
                self.slots[idx] = HookSlot::Memo {
                    deps,
                    value: Value::Null,
                };
                None
            }
        }
    }

    /// Store the value the `useMemo` computation produced (the slot's deps
    /// are already current — recorded at `use_memo`).
    pub fn record_memo(&mut self, value: Value) {
        if let Some(HookSlot::Memo { value: v, .. }) = self.slots.get_mut(self.next_index - 1) {
            *v = value;
        }
    }

    /// `useCallback(deps, value)`: returns the cached handler when deps are
    /// unchanged (same identity — the cached `Value` literally), otherwise
    /// stores and returns the new one. `value` is a `Value::Handler`.
    pub fn use_callback(&mut self, deps: Option<Vec<Value>>, value: Value) -> Value {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            self.slots.push(HookSlot::Callback {
                deps,
                value: value.clone(),
            });
            return value;
        }
        if let HookSlot::Callback {
            deps: d,
            value: cached,
        } = &self.slots[idx]
        {
            if deps_eq(d, &deps) {
                return cached.clone();
            }
        }
        self.slots[idx] = HookSlot::Callback {
            deps,
            value: value.clone(),
        };
        value
    }

    /// Write the state computed by a dispatch (marks the frame dirty).
    pub fn write_state(&mut self, idx: usize, new_value: Value) {
        if let Some(HookSlot::Reducer { state, .. }) = self.slots.get_mut(idx) {
            if *state != new_value {
                *state = new_value;
                self.dirty = true;
            }
        }
    }

    /// `useEffect(effect_body, deps, cleanup)`: records the effect and
    /// whether it should run after this commit (first time, no deps, or
    /// deps changed since the previous run tracked in the slot). Returns
    /// `(should_run, old_cleanup)`: when the deps changed, the PREVIOUS
    /// cleanup (if any) must run immediately before the new setup (React
    /// order). `cleanup` is the effect arrow's VALUE when it returns a
    /// cleanup closure (captured env included); when the deps did NOT
    /// change, the previously-armed cleanup stays armed.
    pub fn use_effect(
        &mut self,
        deps: Option<Vec<Value>>,
        cleanup: Option<EffectBody>,
    ) -> (bool, Option<EffectBody>) {
        let idx = self.next_index;
        self.next_index += 1;
        if idx >= self.slots.len() {
            // First time: create a slot; always run. Nothing to clean.
            self.slots.push(HookSlot::Effect { deps, cleanup });
            return (true, None);
        }
        // Existing slot: compare deps to decide whether the effect re-runs.
        let should_run = match (&deps, &self.slots[idx]) {
            (None, _) => true,
            (Some(_), HookSlot::Effect { deps: None, .. }) => true,
            (Some(d), HookSlot::Effect { deps: Some(p), .. }) => d != p,
            (Some(_), _) => true,
        };
        let old_cleanup = if should_run {
            match &self.slots[idx] {
                HookSlot::Effect { cleanup, .. } => cleanup.clone(),
                _ => None,
            }
        } else {
            None
        };
        if should_run {
            self.slots[idx] = HookSlot::Effect { deps, cleanup };
        }
        (should_run, old_cleanup)
    }

    /// Reset the call cursor for a new render (keeps slots, drops pending).
    /// `pass` is the current render-pass number: a frame whose last pass is
    /// at least one whole pass stale was UNMOUNTED in between — its hook
    /// state is destroyed (React unmount semantics) and re-initializes from
    /// the hooks' initial values on this render.
    pub fn begin_render(&mut self, pass: u64) -> Vec<EffectBody> {
        if let Some(last) = self.last_pass {
            if last + 1 < pass {
                // Unmounted for a full pass: run every armed cleanup (React
                // cleanup-on-unmount) and destroy the state.
                let mut cleanups = Vec::new();
                for slot in &self.slots {
                    if let HookSlot::Effect {
                        cleanup: Some(c), ..
                    } = slot
                    {
                        cleanups.push(c.clone());
                    }
                }
                self.slots.clear();
                self.last_pass = Some(pass);
                self.next_index = 0;
                self.effects.clear();
                return cleanups;
            }
        }
        self.last_pass = Some(pass);
        self.next_index = 0;
        self.effects.clear();
        self.render_count += 1;
        Vec::new()
    }

    /// Drain effects that should run after this commit.
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
}

/// Deps equality: both `None` (no deps — React runs every render; here we
/// treat `None` as "always recompute") or equal vectors. `Some([])` equals
/// only `Some([])` (never recompute), while `None` never equals anything.
fn deps_eq(a: &Option<Vec<Value>>, b: &Option<Vec<Value>>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// A state setter handle. Carries the frame slot index (the frame protocol's
/// callback channel). Cloning/sending it across the ABI boundary is fine
/// because only the index travels — the actual mutation happens in the frame
/// that owns `frame_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setter {
    pub frame_index: usize,
}
