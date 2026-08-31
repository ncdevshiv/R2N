def patch(path, old, new, count=1):
    s = open(path, encoding='utf-8').read()
    assert s.count(old) == count, f"{path}: count={s.count(old)} for {old[:60]!r}"
    s = s.replace(old, new, count)
    open(path, 'w', encoding='utf-8', newline='\n').write(s)
    print(f"patched {path}")

# ---- take_unmounted_cleanups returns EffectJobs ----
patch('crates/r2n-runtime/src/engine.rs', '''                out.extend(frame.take_cleanups());''',
      '''                out.extend(
                    frame.take_cleanups().into_iter().map(EffectJob::Effect),
                );''')

# ---- run_effects -> job runner (effects + promise continuations) ----
patch('crates/r2n-runtime/src/engine.rs', '''fn run_effects(
    effects: &[EffectJob],
    frames: &mut FrameStore,
    host: &mut dyn Host,
    components: &[r2n_ir::runtime::RuntimeComponent],
    strict: bool,
) -> Result<(), RuntimeError> {
    for e in effects {
        let mut env = e.env.clone();
        // Hook handles inside the body (refs, setters) live in the owning
        // component's frame — resolve it by path, not a throwaway.
        let frame = match &e.frame_path {
            Some(p) => frames.get(p),
            None => unreachable!("every EffectBody carries a frame path"),
        };
        if strict {
            // React StrictMode dev double-invoke: setup -> cleanup ->
            // setup, surfacing impure effects.
            run_effect_body(&e.body, &mut env, frame, host, components)?;
            if let Some(cleanup) =
                crate::eval::cleanup_of(&e.body, &env, true, e.frame_path.clone())
            {
                let mut cenv = cleanup.env.clone();
                run_effect_body(&cleanup.body, &mut cenv, frame, host, components)?;
            }
            let mut env2 = e.env.clone();
            run_effect_body(&e.body, &mut env2, frame, host, components)?;
        } else {
            run_effect_body(&e.body, &mut env, frame, host, components)?;
        }
    }
    Ok(())
}''', '''/// Run one drain of runtime jobs: React effects AND promise continuations
/// (M2-T07). Returns the jobs spawned while running (chained .then, the next
/// async segment) — callers loop until the queue is empty (drain_jobs).
#[allow(clippy::too_many_arguments)]
fn run_jobs(
    jobs: &[EffectJob],
    frames: &mut FrameStore,
    host: &mut dyn Host,
    components: &[r2n_ir::runtime::RuntimeComponent],
    strict: bool,
) -> Result<Vec<EffectJob>, RuntimeError> {
    let mut spawned: Vec<EffectJob> = Vec::new();
    for job in jobs {
        match job {
            EffectJob::Effect(eb) => {
                let mut env = eb.env.clone();
                // Hook handles inside the body (refs, setters) live in the
                // owning component's frame — resolve it by path.
                let frame = match &eb.frame_path {
                    Some(p) => frames.get(p),
                    None => frames.get(&[]),
                };
                if strict {
                    // React StrictMode dev double-invoke: setup -> cleanup ->
                    // setup, surfacing impure effects.
                    spawned.extend(run_effect_body(
                        &eb.body,
                        &mut env,
                        frame,
                        host,
                        components,
                    )?);
                    if let Some(cleanup) =
                        crate::eval::cleanup_of(&eb.body, &env, true, eb.frame_path.clone())
                    {
                        let mut cenv = cleanup.env.clone();
                        spawned.extend(run_effect_body(
                            &cleanup.body,
                            &mut cenv,
                            frame,
                            host,
                            components,
                        )?);
                    }
                    let mut env2 = eb.env.clone();
                    spawned.extend(run_effect_body(
                        &eb.body,
                        &mut env2,
                        frame,
                        host,
                        components,
                    )?);
                } else {
                    spawned.extend(run_effect_body(
                        &eb.body,
                        &mut env,
                        frame,
                        host,
                        components,
                    )?);
                }
            }
            EffectJob::Then {
                on_ok,
                on_err,
                env,
                value,
                rejected,
                result,
                frame_path,
            } => {
                let chosen = if *rejected { on_err } else { on_ok };
                match chosen {
                    // Pass-through chaining (no handler / adoption): the
                    // result settles with the same value. An ADOPTING result
                    // (settled but Pending) is completed here via force.
                    None => {
                        crate::eval::force_settle_pub(result, value.clone(), !rejected, &mut spawned);
                    }
                    Some((body, param)) => {
                        let mut cenv = env.clone();
                        cenv.push_scope();
                        cenv.define(param, value.clone());
                        let frame = match frame_path {
                            Some(p) => frames.get(p),
                            None => frames.get(&[]),
                        };
                        match crate::eval::eval(
                            body,
                            &mut cenv,
                            frame,
                            host,
                            components,
                            &mut spawned,
                        ) {
                            Ok(v) => {
                                crate::eval::settle_promise(result, v, true, &mut spawned)
                            }
                            Err(e) => {
                                let reason = e.caught_value();
                                crate::eval::settle_promise(result, reason, false, &mut spawned)
                            }
                        }
                    }
                }
            }
            EffectJob::Resume {
                af,
                seg,
                bind,
                completes,
                call_env,
                result,
                frame_path,
                incoming,
            } => {
                let mut cenv = call_env.clone();
                if let Some(v) = incoming {
                    if *completes {
                        // `return await p` — the resolved value completes.
                        crate::eval::settle_promise(result, v.clone(), true, &mut spawned);
                        continue;
                    }
                    if let Some(b) = bind {
                        cenv.define(b, v.clone());
                    }
                }
                let frame = match frame_path {
                    Some(p) => frames.get(p),
                    None => frames.get(&[]),
                };
                crate::eval::run_async_step(
                    af.clone(),
                    *seg,
                    &mut cenv,
                    frame,
                    host,
                    components,
                    &mut spawned,
                    result.clone(),
                    frame_path.clone(),
                );
            }
        }
    }
    Ok(spawned)
}

/// Drain a job queue until empty (M2-T07): each job's spawned continuations
/// re-enter the queue, so chained .then and multi-await async fns resolve
/// within one drain. Guarded against runaway cycles.
fn drain_jobs(
    queue: Vec<EffectJob>,
    frames: &mut FrameStore,
    host: &mut dyn Host,
    components: &[r2n_ir::runtime::RuntimeComponent],
    strict: bool,
) -> Result<(), RuntimeError> {
    let mut q: std::collections::VecDeque<EffectJob> = queue.into();
    let mut guard = 0;
    while let Some(job) = q.pop_front() {
        guard += 1;
        if guard > 10_000 {
            return Err(RuntimeError::new(
                "job queue exceeded 10000 continuations",
            ));
        }
        let spawned = run_jobs(&[job], frames, host, components, strict)?;
        for j in spawned {
            q.push_back(j);
        }
    }
    Ok(())
}''')

# ---- flush's post-commit drain ----
patch('crates/r2n-runtime/src/engine.rs', '''        if !deferred_effects.is_empty() {
            run_effects(
                &deferred_effects,
                &mut self.frames,
                &mut host,
                &self.template.components,
                self.template.strict_mode,
            )?;
        }''', '''        if !deferred_effects.is_empty() {
            drain_jobs(
                deferred_effects,
                &mut self.frames,
                &mut host,
                &self.template.components,
                self.template.strict_mode,
            )?;
        }''')

# ---- dispatch's effect drain ----
patch('crates/r2n-runtime/src/engine.rs', '''            run_effects(
                &effects,
                frames,
                &mut host,
                components,
                self.template.strict_mode,
            )?;
            Ok(patches)''', '''            drain_jobs(
                effects,
                frames,
                &mut host,
                components,
                self.template.strict_mode,
            )?;
            Ok(patches)''')

# ---- unmount cleanup drain ----
patch('crates/r2n-runtime/src/engine.rs', '''        if !unmount_cleanups.is_empty() {
            run_effects(
                &unmount_cleanups.into_iter().map(EffectJob::Effect).collect::<Vec<_>>(),
                &mut self.frames,
                &mut host,
                &self.template.components,
                self.template.strict_mode,
            )?;
        }''', '''        if !unmount_cleanups.is_empty() {
            drain_jobs(
                unmount_cleanups,
                &mut self.frames,
                &mut host,
                &self.template.components,
                self.template.strict_mode,
            )?;
        }''')

# ---- run_layout_effects -> job-aware ----
patch('crates/r2n-runtime/src/engine.rs', '''fn run_layout_effects(
    effects: &[EffectJob],
    frames: &mut FrameStore,
    host: &mut dyn Host,
    components: &[r2n_ir::runtime::RuntimeComponent],
    strict: bool,
) -> Result<(), RuntimeError> {
    let layout: Vec<&EffectBody> = effects.iter().filter(|e| e.layout).collect();
    for e in layout {
        let mut env = e.env.clone();
        let frame = match &e.frame_path {
            Some(p) => frames.get(p),
            None => unreachable!("every EffectBody carries a frame path"),
        };''', '''fn run_layout_effects(
    effects: &[EffectJob],
    frames: &mut FrameStore,
    host: &mut dyn Host,
    components: &[r2n_ir::runtime::RuntimeComponent],
    strict: bool,
) -> Result<Vec<EffectJob>, RuntimeError> {
    let mut spawned: Vec<EffectJob> = Vec::new();
    let layout: Vec<&EffectJob> = effects
        .iter()
        .filter(|j| matches!(j, EffectJob::Effect(eb) if eb.layout))
        .collect();
    for job in layout {
        let EffectJob::Effect(e) = job else {
            continue;
        };
        let mut env = e.env.clone();
        let frame = match &e.frame_path {
            Some(p) => frames.get(p),
            None => unreachable!("every EffectBody carries a frame path"),
        };''')
