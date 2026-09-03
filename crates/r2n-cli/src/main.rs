//! `r2n` — the R2N command-line tool.
//!
//! Subcommands:
//!   r2n build <file.r2n>        compile to a JSON artifact (the language-neutral output)
//!   r2n render <file.r2n>       compile + run the zero-JS runtime + print the rendered tree
//!   r2n run <file.r2n> [clicks] compile + run, then fire `clicks` real events on the
//!                               first element that has an onClick handler, printing the
//!                               tree after each click — the full reactive loop:
//!                               event → handler → setter → dirty → flush → patch
//!
//! This is a real, working tool (no stubs): `build` emits genuine serialized
//! IR; `render`/`run` execute the actual interpreter, event dispatch, and
//! reconciliation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

use r2n_compiler::{link_source, FsResolver, LinkError};
use r2n_runtime::{NodeId, Renderer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: r2n <build|render|run> <file.r2n> [clicks]");
        exit(2);
    }
    let cmd = args[1].as_str();
    let file = &args[2];
    let src = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {file}: {e}");
            exit(1);
        }
    };

    // Resolve the entry as an absolute path so multi-file `import` specifiers
    // resolve relative to the entry's directory (M2-T09).
    let abs = absolute_path(file);
    let resolver = FsResolver::new(
        Path::new(&abs)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
    );
    let template = match link_source(&src, &abs, &resolver) {
        Ok(t) => t,
        Err(e) => {
            // A parse failure in the entry is best reported with recovery
            // diagnostics (every error + caret); a module-resolution/link
            // failure is already precise enough to print directly.
            if matches!(e, LinkError::Parse(_)) {
                match r2n_compiler::collect_diagnostics(&src) {
                    Ok(all) if !all.is_empty() => {
                        for diag in &all {
                            eprintln!("{diag}\n");
                        }
                        eprintln!("found {} error(s)", all.len());
                    }
                    _ => eprintln!("link error: {e}"),
                }
            } else {
                eprintln!("link error: {e}");
            }
            exit(1);
        }
    };

    match cmd {
        "build" => match r2n_ir::ser::to_json(&template) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("serialize error: {e}");
                exit(1);
            }
        },
        "render" => {
            let mut rt = r2n_runtime::Runtime::new(template);
            let patches = match rt.flush() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("runtime error: {e}");
                    exit(1);
                }
            };
            let mut renderer = r2n_renderer_memory::MemoryRenderer::new();
            renderer.apply(&patches);
            println!("// initial render");
            println!("{}", renderer.render_string());
            println!("// {} patch(es) emitted", patches.len());
            for line in rt.logs() {
                println!("// log: {line}");
            }
        }
        "run" => {
            let clicks: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            let mut rt = r2n_runtime::Runtime::new(template);
            let mut renderer = r2n_renderer_memory::MemoryRenderer::new();
            // step 0: initial render
            let patches = rt.flush().expect("flush");
            renderer.apply(&patches);
            println!("step 0:\n{}\n", renderer.render_string());
            // steps 1..: fire real events on the first clickable node.
            if clicks > 0 {
                let Some(node) = first_clickable(&renderer) else {
                    eprintln!("no element with an onClick handler found; nothing to run");
                    exit(1);
                };
                for step in 1..=clicks {
                    let patches = rt.dispatch(node, "onClick").unwrap_or_else(|e| {
                        eprintln!("dispatch error: {e}");
                        exit(1);
                    });
                    renderer.apply(&patches);
                    println!("// {patches:?}");
                    println!("step {step}:\n{}\n", renderer.render_string());
                }
            }
        }
        other => {
            eprintln!("unknown subcommand '{other}'");
            exit(2);
        }
    }
}

/// The id of the first element carrying an `onClick` handler prop.
fn first_clickable(r: &r2n_renderer_memory::MemoryRenderer) -> Option<NodeId> {
    r.nodes().iter().find_map(|(id, n)| match n {
        r2n_renderer_memory::MemNode::Element { tag, props } => {
            let clickable = tag == "button"
                && props.iter().any(|(k, v)| {
                    k == "onClick" && matches!(v, r2n_runtime::Value::Handler { .. })
                });
            if clickable {
                Some(*id)
            } else {
                None
            }
        }
        _ => None,
    })
}

/// Turn a CLI file argument into an absolute path for deterministic module
/// specifier resolution. We avoid `fs::canonicalize` because it injects the
/// `\\?\` prefix on Windows, which is hostile to the normalized module ids the
/// linker produces.
fn absolute_path(file: &str) -> String {
    let p = Path::new(file);
    if p.is_absolute() {
        p.display().to_string()
    } else {
        std::env::current_dir()
            .map(|d| d.join(file).display().to_string())
            .unwrap_or_else(|_| file.to_string())
    }
}
