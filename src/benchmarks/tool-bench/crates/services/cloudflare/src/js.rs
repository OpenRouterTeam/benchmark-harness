//! Runs agent-supplied Code Mode programs in an embedded QuickJS engine.
//!
//! The real server runs each program in a fresh Workers isolate; here each call gets a fresh
//! QuickJS runtime with the same globals the program can rely on (see `assets/js/`). Limits
//! are deterministic: an operation budget counted by QuickJS's interrupt handler (never the
//! wall clock), a memory cap and a stack cap.

use std::{cell::Cell, rc::Rc};

use rquickjs::{CatchResultExt, CaughtError, Context, Ctx, Function, Runtime, Value as JsValue};
use serde_json::Value;

const PRELUDE: &str = include_str!("../assets/js/prelude.js");
const TRUNCATE: &str = include_str!("../assets/js/truncate.js");
const EXECUTE: &str = include_str!("../assets/js/execute.js");

/// QuickJS calls the interrupt handler about every 10,000 operations; this caps a program
/// at roughly 50 million operations.
const INTERRUPT_BUDGET: u32 = 5_000;
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
const STACK_LIMIT: usize = 1024 * 1024;

pub const CPU_LIMIT_MESSAGE: &str = "Worker exceeded CPU time limit.";

/// What a program produced: the rendered result text, or the error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok(String),
    Err(String),
}

pub enum Mode {
    /// `search`: `spec` is the pinned OpenAPI document; no `cloudflare` binding.
    Search { spec: &'static str },
    /// `execute`: `cloudflare.request()` goes through `fetch`; `accountId` is bound or throws.
    Execute {
        account_id: Option<String>,
        unresolved_message: String,
        fetch: Box<dyn Fn(String) -> String>,
    },
}

pub fn run(mode: Mode, code: &str, truncate: Option<bool>) -> Outcome {
    match run_inner(mode, code, truncate) {
        Ok(outcome) => outcome,
        Err(e) => Outcome::Err(e),
    }
}

fn exception_text(err: CaughtError<'_>) -> String {
    match err {
        CaughtError::Exception(e) => {
            let name: Option<String> = e.get("name").ok();
            let message = e.message().unwrap_or_default();
            match name {
                Some(n) if !n.is_empty() => format!("{n}: {message}"),
                _ => message,
            }
        }
        CaughtError::Value(v) => v
            .as_string()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_else(|| "Uncaught exception".to_owned()),
        CaughtError::Error(e) => e.to_string(),
    }
}

fn eval_script<'js>(ctx: &Ctx<'js>, source: &str) -> Result<JsValue<'js>, String> {
    ctx.eval::<JsValue, _>(source)
        .catch(ctx)
        .map_err(exception_text)
}

fn run_inner(mode: Mode, code: &str, truncate: Option<bool>) -> Result<Outcome, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(STACK_LIMIT);
    let ticks = Rc::new(Cell::new(0u32));
    let counter = ticks.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        counter.set(counter.get() + 1);
        counter.get() > INTERRUPT_BUDGET
    })));
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;

    let is_search = matches!(mode, Mode::Search { .. });
    let setup: Result<(), String> = ctx.with(|ctx| {
        let globals = ctx.globals();
        match mode {
            Mode::Search { spec } => {
                let f = Function::new(ctx.clone(), move || spec.to_owned())
                    .map_err(|e| e.to_string())?;
                globals.set("__spec_text", f).map_err(|e| e.to_string())?;
                globals
                    .set("__cf_fetch", Function::new(ctx.clone(), String::new).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            }
            Mode::Execute {
                fetch,
                account_id,
                unresolved_message,
            } => {
                let f = Function::new(ctx.clone(), move |req: String| fetch(req))
                    .map_err(|e| e.to_string())?;
                globals.set("__cf_fetch", f).map_err(|e| e.to_string())?;
                let prelude = match account_id {
                    Some(id) => format!("const accountId = {};", Value::String(id)),
                    None => format!(
                        "Object.defineProperty(globalThis, \"accountId\", {{ configurable: true, get() {{ throw new Error({}); }} }});",
                        Value::String(unresolved_message)
                    ),
                };
                eval_script(&ctx, PRELUDE)?;
                eval_script(&ctx, TRUNCATE)?;
                eval_script(&ctx, EXECUTE)?;
                eval_script(&ctx, &prelude)?;
                return Ok(());
            }
        }
        eval_script(&ctx, PRELUDE)?;
        eval_script(&ctx, TRUNCATE)?;
        eval_script(&ctx, EXECUTE)?;
        eval_script(
            &ctx,
            "delete globalThis.cloudflare; delete globalThis.__cf_fetch; const spec = JSON.parse(__spec_text()); delete globalThis.__spec_text;",
        )?;
        Ok(())
    });
    setup?;

    // The program, compiled on its own so a syntax error is reported as such.
    let started: Result<(), String> = ctx.with(|ctx| {
        let user =
            eval_script(&ctx, &format!("(\n{code}\n)")).map_err(|e| format!("Uncaught {e}"))?;
        let runner: Function = ctx
            .globals()
            .get(if is_search {
                "__runSearch"
            } else {
                "__runExecute"
            })
            .map_err(|e| e.to_string())?;
        let truncate = match truncate {
            Some(b) => JsValue::new_bool(ctx.clone(), b),
            None => JsValue::new_undefined(ctx.clone()),
        };
        runner
            .call::<_, JsValue>((user, truncate))
            .catch(&ctx)
            .map_err(exception_text)?;
        Ok(())
    });
    if let Err(e) = started {
        if ticks.get() > INTERRUPT_BUDGET {
            return Ok(Outcome::Err(CPU_LIMIT_MESSAGE.to_owned()));
        }
        return Ok(Outcome::Err(e));
    }

    loop {
        match rt.execute_pending_job() {
            Ok(true) => continue,
            Ok(false) => break,
            Err(_) => break,
        }
    }
    if ticks.get() > INTERRUPT_BUDGET {
        return Ok(Outcome::Err(CPU_LIMIT_MESSAGE.to_owned()));
    }

    let outcome: Option<String> = ctx.with(|ctx| ctx.globals().get("__outcome").ok());
    let Some(outcome) = outcome else {
        return Ok(Outcome::Err(
            "The Workers runtime canceled this request because it detected that your Worker's code had hung and would never generate a response.".to_owned(),
        ));
    };
    let parsed: Value = serde_json::from_str(&outcome).map_err(|e| e.to_string())?;
    Ok(if parsed["ok"] == Value::Bool(true) {
        Outcome::Ok(parsed["text"].as_str().unwrap_or_default().to_owned())
    } else {
        Outcome::Err(parsed["message"].as_str().unwrap_or_default().to_owned())
    })
}
