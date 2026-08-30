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
}

#[derive(Debug, Clone)]
enum HookSlot {
    State {
        value: Value,
    },
    Ref {
        value: Value,
    },
    /// `useReducer`: the reducer's params/body and the current state.
    /// The dispatcher evaluates `reducer(state, action)` on each dispatch.
    Reducer {
        params: Vec<String>,
        body: r2n_ir::js::JsExpr,
        state: Value,
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

    /// Write the state computed by a dispatch (marks the frame dirty).
    pub fn write_state(&mut self, idx: usize, new_value: Value) {
        if let Some(HookSlot::Reducer { state, .. }) = self.slots.get_mut(idx) {
            if *state != new_value {
                *state = new_value;
                self.dirty = true;
            }
        }
    }

    /// `useEffect(effect_body, deps)`. Records the effect and whether it should
    /// run after this commit (first time, no deps, or deps changed since the
    /// previous run tracked in the slot). The runtime flushes it after commit.
    pub fn use_effect(&mut self, deps: Option<Vec<Value>>) -> bool {
        let idx = self.next_index;
        self.next_index += 1;
        let (prev_deps, should_run) = if idx >= self.slots.len() {
            // First time: create a slot; always run.
            self.slots.push(HookSlot::Ref { value: Value::Null });
            (None, true)
        } else if let HookSlot::Ref { value } = &self.slots[idx] {
            let prev = if let Value::Array(a) = value {
                Some(a.clone())
            } else {
                None
            };
            let should_run = match (&deps, &prev) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(d), Some(p)) => d != p,
            };
            (prev, should_run)
        } else {
            (None, true)
        };
        // Persist the current deps in the slot so the next render can compare.
        if idx < self.slots.len() {
            if let HookSlot::Ref { value } = &mut self.slots[idx] {
                *value = match &deps {
                    Some(d) => Value::Array(d.clone()),
                    None => Value::Null,
                };
            }
        }
        let _ = prev_deps;
        if should_run {
            self.effects.push(Effect {
                deps,
                prev_deps: None,
            });
        }
        should_run
    }

    /// Reset the call cursor for a new render (keeps slots, drops pending).
    /// `pass` is the current render-pass number: a frame whose last pass is
    /// at least one whole pass stale was UNMOUNTED in between — its hook
    /// state is destroyed (React unmount semantics) and re-initializes from
    /// the hooks' initial values on this render.
    pub fn begin_render(&mut self, pass: u64) {
        if let Some(last) = self.last_pass {
            if last + 1 < pass {
                self.slots.clear();
            }
        }
        self.last_pass = Some(pass);
        self.next_index = 0;
        self.effects.clear();
    }

    /// Drain effects that should run after this commit.
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
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
