//! Script execution (feature `scripting`, QuickJS via `rquickjs`). Scripts define optional
//! functions:
//!
//! ```js
//! function onRequest(req) {      // req: {method, url, httpVersion, headers: {name: value|[values]}, body: string}
//!   req.headers["x-debug"] = "1";
//!   return req;                  // or return {status, headers, body} to respond immediately
//! }
//! function onResponse(req, res) { // res: {status, statusMessage, httpVersion, headers, body}
//!   res.body = res.body.replace("foo", "bar");
//!   return res;
//! }
//! ```
//!
//! Each call runs with a 2s time limit and 64MB memory cap. `console.log` output is captured.
//!
//! ## Contract details
//!
//! * The time limit covers the whole call (runtime start-up, script evaluation and the hook).
//!   QuickJS's interrupt handler stops the script at the deadline, and JS code can't catch
//!   that. It also stops regular expressions (catastrophic backtracking included) and some
//!   long built-in loops such as `indexOf`, but other built-ins can't be interrupted, such as
//!   `Array(2 ** 30).join('')` or a long substring search. So every call also runs on its own
//!   thread, under a hard wall-clock watchdog: the caller gets an error at most 0.5s after the
//!   deadline, even when the script is still stuck. The stuck thread is abandoned and ends
//!   when the native call returns. While a few abandoned threads are still running, new
//!   script calls fail at once instead of piling up more stuck threads. At most a few dozen
//!   scripts run at once; further calls wait for a free slot.
//! * Recursion is limited (deep JS recursion, `JSON.parse` / `JSON.stringify` of deeply nested
//!   data, `console.log` of such values) and raises a catchable `RangeError: Maximum call stack
//!   size exceeded`; it can never overflow the native stack. `JSON.stringify` keeps its
//!   standard behaviour.
//! * The memory cap includes the request/response data handed to the script, so very large
//!   bodies (tens of MB) can exhaust it.
//! * Every call uses a fresh QuickJS runtime and context (no state is shared between calls or
//!   scripts). Top-level `function` declarations and `const`/`let` bindings both work, and
//!   hooks may be `async` (there are no timers or I/O, so awaited promises must settle
//!   synchronously).
//! * `headers` is an object keyed by the original header names (names differing only in case
//!   are grouped under the first spelling); a value is a string, or an array for repeated
//!   headers. Returning the headers object unchanged keeps the exact original header order and
//!   casing; otherwise headers are rebuilt in object order (`null`/`undefined` values drop a
//!   header).
//! * `body` is the decoded body as a string (UTF-8, invalid sequences replaced). When the script
//!   source mentions `bodyBase64`, the exact bytes are also provided as `bodyBase64`; setting it
//!   replaces the body with the decoded bytes. An unchanged `body` keeps the original bytes, so
//!   binary bodies survive scripts that don't touch them. Returned non-string bodies are
//!   serialized as JSON, and a returned `json` property replaces the body with its JSON
//!   serialization (adding `content-type: application/json` when absent).
//! * A hook may mutate its argument and return nothing (`undefined`/`null`).
//! * `onRequest` may change `method`, `url`, `headers` and the body. When the URL changes, the
//!   `Host` header follows the new authority unless the script also set the `host` header to a
//!   new value itself or set `req.preserveHost = true`.
//! * Returning an object with a numeric `status` from `onRequest` responds immediately
//!   ([`ScriptRequestOutcome::Respond`]): `statusMessage` defaults to the canonical reason,
//!   `Date` and a correct `Content-Length` are added, and `HEAD` requests get no body.
//! * In `onResponse`, fields that aren't returned keep their values; changing `status` without
//!   `statusMessage` resets the reason phrase.
//! * `console.log/info/debug/warn/error` lines are captured as `[level] message` (objects are
//!   JSON-serialized), up to 1000 lines of 8 KiB each.
//! * In `onResponse(req, res)`, `req` describes the request as sent (its `body` is empty: the
//!   request body isn't available at that point).
//! * Errors (syntax errors, exceptions, timeouts, memory exhaustion) are returned as
//!   [`RulesError::Script`] with the error name, message and line number, followed by the last
//!   20 console lines printed before the failure (the [`ScriptLog`] is not returned on error).
//! * Hooks are looked up by name: a script whose source doesn't contain `onRequest` /
//!   `onResponse` is skipped for that hook without starting a runtime.
//! * Rule compilation runs the script's top-level code (limit 0.5s) to report errors early.
//!   Top-level code that is stuck in work that can't be interrupted is reported as an error.
//!
//! These functions block the calling thread while the script runs (up to the time limit plus
//! the watchdog's grace period); call them from a blocking-friendly context (e.g.
//! `tokio::task::spawn_blocking`).

use crate::{Result, RulesError};
use akev_core::{RequestHead, ResponseHead};
use bytes::Bytes;

/// The result of running `onRequest`.
#[derive(Clone, Debug)]
pub enum ScriptRequestOutcome {
    /// Continue with the (possibly modified) request.
    Continue { head: RequestHead, body: Bytes },
    /// Respond immediately with this response (upstream is not contacted).
    Respond { head: ResponseHead, body: Bytes },
}

/// Console output captured while a script ran.
#[derive(Clone, Debug, Default)]
pub struct ScriptLog {
    pub lines: Vec<String>,
}

/// Run the script's `onRequest(req)` hook (if defined). Scripts without the hook continue
/// with the request unchanged.
pub fn run_on_request(
    source: &str,
    head: RequestHead,
    body: Bytes,
) -> Result<(ScriptRequestOutcome, ScriptLog)> {
    imp::run_on_request(source, head, body)
}

/// Run the script's `onResponse(req, res)` hook (if defined). Scripts without the hook return
/// the response unchanged.
pub fn run_on_response(
    source: &str,
    request: &RequestHead,
    head: ResponseHead,
    body: Bytes,
) -> Result<(ResponseHead, Bytes, ScriptLog)> {
    imp::run_on_response(source, request, head, body)
}

/// Compile-time validation used by the rules engine: syntax errors, errors thrown by top-level
/// code and scripts without any hook are reported.
pub(crate) fn check_syntax(source: &str) -> std::result::Result<(), String> {
    imp::check_syntax(source)
}

#[cfg(not(feature = "scripting"))]
mod imp {
    use super::*;

    const DISABLED: &str = "scripting support is not compiled into this build";

    pub(super) fn run_on_request(
        _: &str,
        _: RequestHead,
        _: Bytes,
    ) -> Result<(ScriptRequestOutcome, ScriptLog)> {
        Err(RulesError::Script(DISABLED.to_string()))
    }

    pub(super) fn run_on_response(
        _: &str,
        _: &RequestHead,
        _: ResponseHead,
        _: Bytes,
    ) -> Result<(ResponseHead, Bytes, ScriptLog)> {
        Err(RulesError::Script(DISABLED.to_string()))
    }

    pub(super) fn check_syntax(_: &str) -> std::result::Result<(), String> {
        Err(DISABLED.to_string())
    }
}

#[cfg(feature = "scripting")]
mod imp {
    use super::*;
    use crate::respond::{decode_base64, finalize_local_response};
    use crate::supervisor::{self, Failure};
    use crate::transform::{validate_header_name, validate_header_value};
    use crate::urlutil::set_request_url;
    use akev_core::Headers;
    use akev_core::http::canonical_reason;
    use base64::Engine;
    use rquickjs::context::EvalOptions;
    use rquickjs::{Context, Ctx, Function, Persistent, Runtime, Value as JsValue};
    use serde_json::{Map, Value};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// Time budget per call.
    pub(super) const TIME_LIMIT: Duration = Duration::from_secs(2);
    /// Memory cap per runtime.
    pub(super) const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
    /// Native stack QuickJS may use before it throws `RangeError: Maximum call stack size
    /// exceeded` (QuickJS's default). Scripts run on threads with a much larger stack
    /// ([`supervisor::THREAD_STACK_SIZE`]), so the engine always stops first.
    pub(super) const JS_STACK_LIMIT: usize = 1024 * 1024;
    const SYNTAX_CHECK_LIMIT: Duration = Duration::from_millis(500);
    const MAX_LOG_LINES: usize = 1000;
    const MAX_LOG_LINE: usize = 8 * 1024;

    /// Body of `function (sink, withKeysSource)` that installs `console` and replaces
    /// `JSON.stringify`; it returns the identity replacer used by [`Sandbox::stringify`]. It is
    /// compiled with the `Function` constructor before any user code runs, so its stack frames
    /// are reported as `<input>` and never mistaken for lines of the user's script
    /// (`eval_script`).
    ///
    /// QuickJS's native serializer recurses into nested arrays and objects without checking
    /// the stack, so stringifying a value nested ~10k levels deep (which `JSON.parse` accepts)
    /// would overflow the native stack and abort the process. The replacement always gives the
    /// native serializer a replacer *function*. The serializer calls it for every value, and
    /// every call goes through QuickJS's stack check, which throws a catchable `RangeError`
    /// instead. An identity replacer changes nothing, and a user's replacer function is passed
    /// on unchanged. An array replacer (a property list) can't be combined with a function, so
    /// that case uses [`STRINGIFY_WITH_KEYS`], compiled on first use.
    const PRELUDE: &str = r#"
  var nativeStringify = JSON.stringify;
  var isArray = Array.isArray;
  var apply = Reflect.apply;
  var defineProperty = Object.defineProperty;
  var makeFunction = Function;
  var arrayJoin = Array.prototype.join;
  var arrayMap = Array.prototype.map;
  // Captured before user code runs, which may replace globals.
  var builtins = {
    __proto__: null,
    nativeStringify: nativeStringify, isArray: isArray, apply: apply,
    defineProperty: defineProperty, createObject: Object.create, toStringOf: String,
    TypeErrorOf: TypeError, objectTag: Object.prototype.toString, arrayJoin: arrayJoin,
    stringSlice: String.prototype.slice, stringRepeat: String.prototype.repeat,
    mathMin: Math.min, mathTrunc: Math.trunc, numberValue: Number.prototype.valueOf,
    stringValue: String.prototype.valueOf, booleanValue: Boolean.prototype.valueOf,
    bigintValue: BigInt.prototype.valueOf
  };
  var stringifyWithKeys = null;

  function keep(key, value) { return value; }

  function stringify(value, replacer, space) {
    if (typeof replacer === 'function') return nativeStringify(value, replacer, space);
    if (isArray(replacer)) {
      if (stringifyWithKeys === null) stringifyWithKeys = makeFunction('b', withKeysSource)(builtins);
      return stringifyWithKeys(value, replacer, space);
    }
    return nativeStringify(value, keep, space);
  }
  defineProperty(JSON, 'stringify', { __proto__: null, value: stringify, writable: true, enumerable: false, configurable: true });

  function fmt(v) {
    if (typeof v === 'string') return v;
    if (v instanceof Error) return v.stack ? (String(v) + '\n' + v.stack).replace(/\s+$/, '') : String(v);
    if (typeof v === 'function') return '[Function' + (v.name ? ': ' + v.name : '') + ']';
    if (typeof v === 'undefined') return 'undefined';
    if (typeof v === 'bigint') return v.toString() + 'n';
    if (typeof v === 'symbol') return v.toString();
    try { var s = stringify(v); return s === undefined ? String(v) : s; } catch (e) {
      try { return String(v); } catch (e2) { return '[unprintable value]'; }
    }
  }
  function make(level) {
    return function () { sink(level, apply(arrayJoin, apply(arrayMap, arguments, [fmt]), [' '])); };
  }
  globalThis.console = {
    log: make('log'), info: make('info'), debug: make('debug'), trace: make('debug'),
    warn: make('warn'), error: make('error')
  };
  return keep;
"#;

    /// Body of `function (b)` (`b`: built-ins captured by [`PRELUDE`]) returning
    /// `JSON.stringify(value, propertyList, space)` implemented in JS per ECMA-262.
    const STRINGIFY_WITH_KEYS: &str = r#"
  var nativeStringify = b.nativeStringify, isArray = b.isArray, apply = b.apply;
  var defineProperty = b.defineProperty, createObject = b.createObject, toStringOf = b.toStringOf;
  var TypeErrorOf = b.TypeErrorOf, objectTag = b.objectTag, arrayJoin = b.arrayJoin;
  var stringSlice = b.stringSlice, stringRepeat = b.stringRepeat, mathMin = b.mathMin;
  var mathTrunc = b.mathTrunc, numberValue = b.numberValue, stringValue = b.stringValue;
  var booleanValue = b.booleanValue, bigintValue = b.bigintValue;

  // 'number' / 'string' / 'boolean' / 'bigint' for primitive wrapper objects, else ''.
  function primitiveOf(v) {
    var probe;
    switch (apply(objectTag, v, [])) {
      case '[object Number]': probe = numberValue; break;
      case '[object String]': probe = stringValue; break;
      case '[object Boolean]': probe = booleanValue; break;
      case '[object BigInt]': probe = bigintValue; break;
      default: return '';
    }
    try { return typeof apply(probe, v, []); } catch (e) { return ''; }
  }

  return function stringifyWithKeys(value, replacer, space) {
    var keys = [];
    var seen = createObject(null);
    var count = replacer.length;
    for (var i = 0; i < count; i++) {
      var entry = replacer[i];
      var item = undefined;
      if (typeof entry === 'string') {
        item = entry;
      } else if (typeof entry === 'number') {
        item = toStringOf(entry);
      } else if (typeof entry === 'object' && entry !== null) {
        var entryKind = primitiveOf(entry);
        if (entryKind === 'string' || entryKind === 'number') item = toStringOf(entry);
      }
      if (item !== undefined && seen[item] !== true) {
        seen[item] = true;
        keys[keys.length] = item;
      }
    }
    if (typeof space === 'object' && space !== null) {
      var spaceKind = primitiveOf(space);
      if (spaceKind === 'number') space = +space;
      else if (spaceKind === 'string') space = toStringOf(space);
    }
    var gap = '';
    if (typeof space === 'number') {
      var width = mathMin(10, mathTrunc(space));
      if (width >= 1) gap = apply(stringRepeat, ' ', [width]);
    } else if (typeof space === 'string') {
      gap = apply(stringSlice, space, [0, 10]);
    }
    var stack = [];
    var indent = '';

    function str(key, holder) {
      var v = holder[key];
      if ((typeof v === 'object' && v !== null) || typeof v === 'bigint') {
        var toJSON = v.toJSON;
        if (typeof toJSON === 'function') v = apply(toJSON, v, [key]);
      }
      if (typeof v === 'object' && v !== null) {
        var kind = primitiveOf(v);
        if (kind === 'number') v = +v;
        else if (kind === 'string') v = toStringOf(v);
        else if (kind === 'boolean') v = apply(booleanValue, v, []);
        else if (kind === 'bigint') v = apply(bigintValue, v, []);
      }
      if (v === null) return 'null';
      if (v === true) return 'true';
      if (v === false) return 'false';
      if (typeof v === 'string') return nativeStringify(v);
      if (typeof v === 'number') return v === v && v !== 1 / 0 && v !== -1 / 0 ? toStringOf(v) : 'null';
      if (typeof v === 'bigint') throw new TypeErrorOf('BigInt are forbidden in JSON.stringify');
      if (typeof v === 'object') return isArray(v) ? serializeArray(v) : serializeObject(v);
      return undefined;
    }
    function enter(v) {
      for (var i = 0; i < stack.length; i++) {
        if (stack[i] === v) throw new TypeErrorOf('circular reference');
      }
      stack[stack.length] = v;
    }
    function finish(open, close, parts, stepback) {
      var out;
      if (parts.length === 0) out = open + close;
      else if (gap === '') out = open + apply(arrayJoin, parts, [',']) + close;
      else out = open + '\n' + indent + apply(arrayJoin, parts, [',\n' + indent]) + '\n' + stepback + close;
      stack.length = stack.length - 1;
      indent = stepback;
      return out;
    }
    function serializeObject(v) {
      enter(v);
      var stepback = indent;
      indent = indent + gap;
      var parts = [];
      for (var i = 0; i < keys.length; i++) {
        var s = str(keys[i], v);
        if (s !== undefined) parts[parts.length] = nativeStringify(keys[i]) + (gap === '' ? ':' : ': ') + s;
      }
      return finish('{', '}', parts, stepback);
    }
    function serializeArray(v) {
      enter(v);
      var stepback = indent;
      indent = indent + gap;
      var parts = [];
      var length = v.length;
      for (var i = 0; i < length; i++) {
        var s = str(toStringOf(i), v);
        parts[parts.length] = s === undefined ? 'null' : s;
      }
      return finish('[', ']', parts, stepback);
    }
    var wrapper = {};
    defineProperty(wrapper, '', { __proto__: null, value: value, writable: true, enumerable: true, configurable: true });
    return str('', wrapper);
  };
"#;

    /// Outcome of invoking a hook.
    enum HookResult {
        /// The script doesn't define the hook.
        Missing,
        /// JSON of the returned (or mutated) value; `None` if it isn't JSON-serializable.
        Returned(Option<Value>),
    }

    struct Sandbox {
        // Field order matters: JS values must drop before the context, and the context before
        // its runtime.
        /// The prelude's identity replacer (see [`PRELUDE`]).
        keep: Persistent<Function<'static>>,
        context: Context,
        _runtime: Runtime,
        timed_out: Rc<Cell<bool>>,
        logs: Rc<RefCell<Vec<String>>>,
    }

    impl Sandbox {
        /// A fresh runtime whose interrupt handler stops scripts at `deadline`. Must be created
        /// (and used) on the thread that runs the script: QuickJS measures its stack limit from
        /// the current stack position.
        fn new(deadline: Instant) -> std::result::Result<Self, String> {
            let runtime =
                Runtime::new().map_err(|e| format!("failed to start script runtime: {e}"))?;
            runtime.set_memory_limit(MEMORY_LIMIT);
            runtime.set_max_stack_size(JS_STACK_LIMIT);
            let timed_out = Rc::new(Cell::new(false));
            let flag = Rc::clone(&timed_out);
            runtime.set_interrupt_handler(Some(Box::new(move || {
                if flag.get() || Instant::now() >= deadline {
                    flag.set(true);
                    true
                } else {
                    false
                }
            })));
            let context = Context::full(&runtime)
                .map_err(|e| format!("failed to create script context: {e}"))?;
            let logs = Rc::new(RefCell::new(Vec::new()));
            let sink = Rc::clone(&logs);
            let installed = context.with(|ctx| -> rquickjs::Result<_> {
                let log = Function::new(ctx.clone(), move |level: String, message: String| {
                    push_log(&sink, &level, message);
                })?;
                let constructor: Function = ctx.globals().get("Function")?;
                let prelude: Function = constructor.call(("sink", "withKeysSource", PRELUDE))?;
                let keep: Function = prelude.call((log, STRINGIFY_WITH_KEYS))?;
                Ok(Persistent::save(&ctx, keep))
            });
            let keep =
                installed.map_err(|e| format!("failed to initialise script console: {e}"))?;
            Ok(Sandbox {
                keep,
                context,
                _runtime: runtime,
                timed_out,
                logs,
            })
        }

        /// `JSON.stringify(value)` for values handed back to Rust, without the native
        /// serializer's unchecked recursion (see [`PRELUDE`]) and independent of globals the
        /// script may have replaced.
        fn stringify<'js>(
            &self,
            ctx: &Ctx<'js>,
            value: JsValue<'js>,
        ) -> rquickjs::Result<Option<rquickjs::String<'js>>> {
            let keep = self.keep.clone().restore(ctx)?;
            ctx.json_stringify_replacer(value, keep)
        }

        fn take_logs(&self) -> ScriptLog {
            ScriptLog {
                lines: std::mem::take(&mut *self.logs.borrow_mut()),
            }
        }

        /// Evaluate the user script, then call `hook` with JSON arguments. When the hook
        /// returns `undefined`/`null`, the argument at `mutable_arg` is returned instead.
        fn call(
            &self,
            source: &str,
            hook: &str,
            args: &[String],
            mutable_arg: usize,
        ) -> std::result::Result<HookResult, String> {
            self.context.with(|ctx| {
                eval_script(&ctx, source).map_err(|e| self.describe(&ctx, e))?;
                let lookup = format!("typeof {hook} === 'function' ? {hook} : undefined");
                let f: JsValue = ctx.eval(lookup).map_err(|e| self.describe(&ctx, e))?;
                let Some(f) = f.into_function() else {
                    return Ok(HookResult::Missing);
                };
                let mut js_args = Vec::with_capacity(args.len());
                for a in args {
                    js_args.push(
                        ctx.json_parse(a.as_str())
                            .map_err(|e| self.describe(&ctx, e))?,
                    );
                }
                let ret: JsValue = match js_args.as_slice() {
                    [a] => f.call((a.clone(),)),
                    [a, b] => f.call((a.clone(), b.clone())),
                    _ => return Err("internal error: unexpected hook arity".to_string()),
                }
                .map_err(|e| self.describe(&ctx, e))?;
                let ret = match ret.as_promise() {
                    Some(p) => p.finish::<JsValue>().map_err(|e| self.describe(&ctx, e))?,
                    None => ret,
                };
                let out = if ret.is_undefined() || ret.is_null() {
                    match js_args.get(mutable_arg) {
                        Some(arg) => arg.clone(),
                        None => return Ok(HookResult::Returned(None)),
                    }
                } else {
                    ret
                };
                let json = self
                    .stringify(&ctx, out)
                    .map_err(|e| self.describe(&ctx, e))?;
                let Some(json) = json else {
                    return Ok(HookResult::Returned(None));
                };
                let json = json.to_string().map_err(|e| e.to_string())?;
                let value = serde_json::from_str(&json)
                    .map_err(|e| format!("hook returned invalid data: {e}"))?;
                Ok(HookResult::Returned(Some(value)))
            })
        }

        fn describe(&self, ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
            if self.timed_out.get() {
                return format!(
                    "script exceeded the {}s CPU time limit",
                    TIME_LIMIT.as_secs()
                );
            }
            match err {
                rquickjs::Error::Exception => self.describe_exception(ctx.catch()),
                rquickjs::Error::Allocation => memory_error(),
                rquickjs::Error::WouldBlock => {
                    "the hook returned a promise that never settled (timers and I/O are not available)".to_string()
                }
                other => other.to_string(),
            }
        }

        /// `Name: message (line N, column M)` from a thrown value.
        fn describe_exception(&self, value: JsValue<'_>) -> String {
            if let Some(obj) = value.as_object() {
                let get = |key: &str| obj.get::<_, Option<String>>(key).ok().flatten();
                let name = get("name");
                let message = get("message");
                if name.is_some() || message.is_some() {
                    let mut out = match (name, message) {
                        (Some(n), Some(m)) if !m.is_empty() => format!("{n}: {m}"),
                        (Some(n), _) => n,
                        (None, Some(m)) => m,
                        (None, None) => String::new(),
                    };
                    if out.contains("out of memory") {
                        return memory_error();
                    }
                    if let Some(loc) = get("stack").as_deref().and_then(script_location) {
                        out.push_str(&format!(" ({loc})"));
                    }
                    return out;
                }
            }
            if value.is_null() {
                // QuickJS can't allocate an exception object once memory is exhausted.
                return memory_error();
            }
            let ctx = value.ctx().clone();
            match self.stringify(&ctx, value) {
                Ok(Some(s)) => format!("uncaught exception: {}", s.to_string().unwrap_or_default()),
                _ => "uncaught exception".to_string(),
            }
        }
    }

    fn memory_error() -> String {
        format!(
            "script exceeded the {} MB memory limit",
            MEMORY_LIMIT / (1024 * 1024)
        )
    }

    fn eval_script<'js>(ctx: &Ctx<'js>, source: &str) -> rquickjs::Result<()> {
        let mut opts = EvalOptions::default();
        opts.global = true;
        opts.strict = false;
        ctx.eval_with_options::<(), _>(source, opts)
    }

    fn push_log(logs: &RefCell<Vec<String>>, level: &str, mut message: String) {
        let mut logs = logs.borrow_mut();
        if logs.len() > MAX_LOG_LINES {
            return;
        }
        if logs.len() == MAX_LOG_LINES {
            logs.push("[akev] further console output truncated".to_string());
            return;
        }
        if message.len() > MAX_LOG_LINE {
            let mut cut = MAX_LOG_LINE;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push('…');
        }
        logs.push(format!("[{level}] {message}"));
    }

    /// Extract `line N[, column M]` for the user script from a QuickJS stack trace.
    fn script_location(stack: &str) -> Option<String> {
        let idx = stack.find("eval_script:")?;
        let rest = &stack[idx + "eval_script:".len()..];
        let line: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if line.is_empty() {
            return None;
        }
        let after = &rest[line.len()..];
        let col: String = after
            .strip_prefix(':')
            .map(|c| c.chars().take_while(char::is_ascii_digit).collect())
            .unwrap_or_default();
        Some(if col.is_empty() {
            format!("line {line}")
        } else {
            format!("line {line}, column {col}")
        })
    }

    pub(super) fn check_syntax(source: &str) -> std::result::Result<(), String> {
        let source = source.to_string();
        let checked = supervisor::global().run(SYNTAX_CHECK_LIMIT, move |deadline| {
            check_syntax_until(&source, deadline)
        });
        match checked {
            Ok(result) => result,
            // The check can't run right now. Accept the rule: a broken script still fails when
            // it runs.
            Err(Failure::Busy(reason)) => {
                tracing::warn!(%reason, "script rule accepted without checking it");
                Ok(())
            }
            // Unlike plain slow code (stopped by the interrupt handler and reported at run
            // time), this would leave a stuck thread behind on every call.
            Err(Failure::Abandoned) => Err(format!(
                "script error: top-level code didn't stop at the {}ms time limit: it is stuck in a \
                 built-in function that can't be interrupted",
                SYNTAX_CHECK_LIMIT.as_millis()
            )),
            Err(Failure::Crashed(e)) => Err(format!("script error: {e}")),
        }
    }

    fn check_syntax_until(source: &str, deadline: Instant) -> std::result::Result<(), String> {
        let sandbox = Sandbox::new(deadline)?;
        let result = sandbox.context.with(|ctx| match eval_script(&ctx, source) {
            Ok(()) => {
                let has_hook: bool = ctx
                    .eval("typeof onRequest === 'function' || typeof onResponse === 'function'")
                    .unwrap_or(true);
                if has_hook {
                    Ok(())
                } else {
                    Err("script defines neither onRequest nor onResponse".to_string())
                }
            }
            // Long-running top-level code isn't a syntax problem; it fails at run time.
            Err(_) if sandbox.timed_out.get() => Ok(()),
            Err(e) => Err(sandbox.describe(&ctx, e)),
        });
        result.map_err(|e| format!("script error: {e}"))
    }

    // --------------------------------------------------------------------------------------
    // Marshalling
    // --------------------------------------------------------------------------------------

    fn headers_to_json(headers: &Headers) -> Value {
        let mut map = Map::new();
        let mut names: Vec<(String, String)> = Vec::new();
        for (name, value) in headers.iter() {
            let value = Value::String(value.to_string());
            match names
                .iter()
                .find(|(lower, _)| lower.eq_ignore_ascii_case(name))
            {
                Some((_, key)) => match map.get_mut(key.as_str()) {
                    Some(Value::Array(items)) => items.push(value),
                    Some(existing) => {
                        let first = existing.take();
                        *existing = Value::Array(vec![first, value]);
                    }
                    None => {}
                },
                None => {
                    names.push((name.to_ascii_lowercase(), name.to_string()));
                    map.insert(name.to_string(), value);
                }
            }
        }
        Value::Object(map)
    }

    fn scalar_to_string(v: &Value) -> Option<String> {
        match v {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }

    fn headers_from_json(
        value: &Value,
        original: &Headers,
        original_json: &Value,
    ) -> std::result::Result<Headers, String> {
        if value == original_json {
            return Ok(original.clone());
        }
        let obj = value.as_object().ok_or("`headers` must be an object")?;
        let mut out = Headers::with_capacity(obj.len());
        for (name, v) in obj {
            let values: Vec<String> = match v {
                Value::Array(items) => items.iter().filter_map(scalar_to_string).collect(),
                other => scalar_to_string(other).into_iter().collect(),
            };
            if values.is_empty() {
                continue;
            }
            validate_header_name(name)?;
            for v in values {
                validate_header_value(name, &v)?;
                out.append(name.clone(), v);
            }
        }
        Ok(out)
    }

    /// The original body as handed to the script.
    struct BodyIn {
        bytes: Bytes,
        text: String,
        base64: Option<String>,
    }

    impl BodyIn {
        fn new(bytes: Bytes, with_base64: bool) -> Self {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let base64 =
                with_base64.then(|| base64::engine::general_purpose::STANDARD.encode(&bytes));
            BodyIn {
                bytes,
                text,
                base64,
            }
        }

        fn insert_into(&self, map: &mut Map<String, Value>) {
            map.insert("body".into(), Value::String(self.text.clone()));
            if let Some(b) = &self.base64 {
                map.insert("bodyBase64".into(), Value::String(b.clone()));
            }
        }
    }

    /// The body selected by a returned object. `None` = keep the original.
    /// The flag reports whether it came from `json` (JSON content type wanted).
    fn body_from_json(
        obj: &Map<String, Value>,
        original: &BodyIn,
    ) -> std::result::Result<Option<(Bytes, bool)>, String> {
        if let Some(json) = obj.get("json") {
            let bytes = serde_json::to_vec(json).map_err(|e| e.to_string())?;
            return Ok(Some((Bytes::from(bytes), true)));
        }
        if let Some(Value::String(b64)) = obj.get("bodyBase64")
            && original.base64.as_deref() != Some(b64.as_str())
        {
            return Ok(Some((
                Bytes::from(decode_base64(b64).map_err(|_| "`bodyBase64` is not valid base64")?),
                false,
            )));
        }
        match obj.get("body") {
            None => Ok(None),
            Some(Value::String(s)) if *s == original.text => Ok(None),
            Some(Value::String(s)) => Ok(Some((Bytes::copy_from_slice(s.as_bytes()), false))),
            Some(Value::Null) => Ok(Some((Bytes::new(), false))),
            Some(other) => Ok(Some((
                Bytes::from(serde_json::to_vec(other).map_err(|e| e.to_string())?),
                true,
            ))),
        }
    }

    fn ensure_json_content_type(headers: &mut Headers) {
        if !headers.contains("content-type") {
            headers.append("content-type", "application/json");
        }
    }

    fn parse_status(v: &Value) -> std::result::Result<u16, String> {
        v.as_u64()
            .or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
            .and_then(|s| u16::try_from(s).ok())
            .filter(|s| (100..=999).contains(s))
            .ok_or_else(|| format!("invalid status {v}"))
    }

    fn wants_base64(source: &str) -> bool {
        source.contains("bodyBase64")
    }

    fn request_json(head: &RequestHead, body: &BodyIn) -> (Value, Value) {
        let headers = headers_to_json(&head.headers);
        let mut map = Map::new();
        map.insert("method".into(), Value::String(head.method.clone()));
        map.insert("url".into(), Value::String(head.url.clone()));
        map.insert(
            "httpVersion".into(),
            Value::String(head.http_version.as_str().to_string()),
        );
        map.insert("headers".into(), headers.clone());
        body.insert_into(&mut map);
        (Value::Object(map), headers)
    }

    fn response_json(head: &ResponseHead, body: &BodyIn) -> (Value, Value) {
        let headers = headers_to_json(&head.headers);
        let mut map = Map::new();
        map.insert("status".into(), Value::from(head.status));
        map.insert(
            "statusMessage".into(),
            Value::String(head.status_message.clone()),
        );
        map.insert(
            "httpVersion".into(),
            Value::String(head.http_version.as_str().to_string()),
        );
        map.insert("headers".into(), headers.clone());
        body.insert_into(&mut map);
        (Value::Object(map), headers)
    }

    fn script_err(e: String) -> RulesError {
        RulesError::Script(e)
    }

    /// Lines of console output kept in error messages (the log is otherwise lost on error).
    const MAX_ERROR_LOG_LINES: usize = 20;

    /// A script error including the last console lines printed before it.
    fn failure(e: String, logs: &ScriptLog) -> RulesError {
        if logs.lines.is_empty() {
            return RulesError::Script(e);
        }
        let start = logs.lines.len().saturating_sub(MAX_ERROR_LOG_LINES);
        RulesError::Script(format!(
            "{e}\nconsole output before the error:\n{}",
            logs.lines[start..].join("\n")
        ))
    }

    /// Run a hook call on a supervised script thread with the standard time limit.
    fn supervised<T: Send + 'static>(
        job: impl FnOnce(Instant) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        match supervisor::global().run(TIME_LIMIT, job) {
            Ok(result) => result,
            Err(Failure::Busy(reason)) => Err(RulesError::Script(reason)),
            Err(Failure::Abandoned) => Err(RulesError::Script(format!(
                "script exceeded the {}s CPU time limit inside a built-in function that can't be \
                 interrupted (the call was abandoned and runs on in the \
                 background until it returns)",
                TIME_LIMIT.as_secs()
            ))),
            Err(Failure::Crashed(e)) => Err(RulesError::Script(e)),
        }
    }

    pub(super) fn run_on_request(
        source: &str,
        head: RequestHead,
        body: Bytes,
    ) -> Result<(ScriptRequestOutcome, ScriptLog)> {
        if !source.contains("onRequest") {
            // No hook to call: skip creating a runtime altogether.
            return Ok((
                ScriptRequestOutcome::Continue { head, body },
                ScriptLog::default(),
            ));
        }
        let source = source.to_string();
        supervised(move |deadline| run_on_request_until(&source, head, body, deadline))
    }

    fn run_on_request_until(
        source: &str,
        head: RequestHead,
        body: Bytes,
        deadline: Instant,
    ) -> Result<(ScriptRequestOutcome, ScriptLog)> {
        let sandbox = Sandbox::new(deadline).map_err(script_err)?;
        let body_in = BodyIn::new(body, wants_base64(source));
        let (req_json, headers_json) = request_json(&head, &body_in);
        let result = sandbox.call(source, "onRequest", &[req_json.to_string()], 0);
        let logs = sandbox.take_logs();
        let returned = match result.map_err(|e| failure(e, &logs))? {
            HookResult::Missing | HookResult::Returned(None) => {
                return Ok((
                    ScriptRequestOutcome::Continue {
                        head,
                        body: body_in.bytes,
                    },
                    logs,
                ));
            }
            HookResult::Returned(Some(v)) => v,
        };
        let obj = returned.as_object().ok_or_else(|| {
            script_err("onRequest must return an object, undefined or null".to_string())
        })?;

        if obj.contains_key("status") || obj.contains_key("statusCode") {
            let status = parse_status(
                obj.get("status")
                    .or_else(|| obj.get("statusCode"))
                    .unwrap_or(&Value::Null),
            )
            .map_err(|e| script_err(format!("onRequest response: {e}")))?;
            let mut headers = match obj.get("headers") {
                Some(h) => {
                    headers_from_json(h, &Headers::new(), &Value::Null).map_err(script_err)?
                }
                None => Headers::new(),
            };
            let empty = BodyIn {
                bytes: Bytes::new(),
                text: String::new(),
                base64: None,
            };
            let (bytes, json) = body_from_json(obj, &empty)
                .map_err(script_err)?
                .unwrap_or_default();
            if json {
                ensure_json_content_type(&mut headers);
            }
            let message = obj.get("statusMessage").and_then(Value::as_str);
            let (rhead, rbody) = finalize_local_response(status, message, headers, bytes, &head);
            return Ok((
                ScriptRequestOutcome::Respond {
                    head: rhead,
                    body: rbody,
                },
                logs,
            ));
        }

        let mut new_head = head.clone();
        if let Some(m) = obj.get("method").and_then(Value::as_str) {
            let m = m.trim().to_ascii_uppercase();
            if m.is_empty() {
                return Err(script_err("onRequest returned an empty method".to_string()));
            }
            new_head.method = m;
        }
        if let Some(h) = obj.get("headers") {
            new_head.headers =
                headers_from_json(h, &head.headers, &headers_json).map_err(script_err)?;
        }
        let mut new_body = body_in.bytes.clone();
        if let Some((bytes, json)) = body_from_json(obj, &body_in).map_err(script_err)? {
            new_body = bytes;
            if json {
                ensure_json_content_type(&mut new_head.headers);
            }
        }
        if let Some(url) = obj.get("url").and_then(Value::as_str)
            && url != head.url
        {
            let new_host = new_head.headers.get("host");
            let host_edited = new_host.is_some() && new_host != head.headers.get("host");
            let preserve = host_edited
                || obj
                    .get("preserveHost")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            set_request_url(&mut new_head, url, preserve)
                .map_err(|e| script_err(format!("onRequest: {e}")))?;
        }
        Ok((
            ScriptRequestOutcome::Continue {
                head: new_head,
                body: new_body,
            },
            logs,
        ))
    }

    pub(super) fn run_on_response(
        source: &str,
        request: &RequestHead,
        head: ResponseHead,
        body: Bytes,
    ) -> Result<(ResponseHead, Bytes, ScriptLog)> {
        if !source.contains("onResponse") {
            return Ok((head, body, ScriptLog::default()));
        }
        let source = source.to_string();
        let request = request.clone();
        supervised(move |deadline| run_on_response_until(&source, &request, head, body, deadline))
    }

    fn run_on_response_until(
        source: &str,
        request: &RequestHead,
        head: ResponseHead,
        body: Bytes,
        deadline: Instant,
    ) -> Result<(ResponseHead, Bytes, ScriptLog)> {
        let sandbox = Sandbox::new(deadline).map_err(script_err)?;
        let with_b64 = wants_base64(source);
        let body_in = BodyIn::new(body, with_b64);
        let (res_json, headers_json) = response_json(&head, &body_in);
        let req_body = BodyIn {
            bytes: Bytes::new(),
            text: String::new(),
            base64: with_b64.then(String::new),
        };
        let (req_json, _) = request_json(request, &req_body);
        let result = sandbox.call(
            source,
            "onResponse",
            &[req_json.to_string(), res_json.to_string()],
            1,
        );
        let logs = sandbox.take_logs();
        let returned = match result.map_err(|e| failure(e, &logs))? {
            HookResult::Missing | HookResult::Returned(None) => {
                return Ok((head, body_in.bytes, logs));
            }
            HookResult::Returned(Some(v)) => v,
        };
        let obj = returned.as_object().ok_or_else(|| {
            script_err("onResponse must return an object, undefined or null".to_string())
        })?;
        let mut new_head = head.clone();
        if let Some(s) = obj.get("status").or_else(|| obj.get("statusCode")) {
            new_head.status =
                parse_status(s).map_err(|e| script_err(format!("onResponse: {e}")))?;
        }
        match obj.get("statusMessage").and_then(Value::as_str) {
            Some(m) if m != head.status_message => m.clone_into(&mut new_head.status_message),
            _ if new_head.status != head.status => {
                canonical_reason(new_head.status).clone_into(&mut new_head.status_message);
            }
            _ => {}
        }
        if let Some(h) = obj.get("headers") {
            new_head.headers =
                headers_from_json(h, &head.headers, &headers_json).map_err(script_err)?;
        }
        let mut new_body = body_in.bytes.clone();
        if let Some((bytes, json)) = body_from_json(obj, &body_in).map_err(script_err)? {
            new_body = bytes;
            if json {
                ensure_json_content_type(&mut new_head.headers);
            }
        }
        Ok((new_head, new_body, logs))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn stack_location_parsing() {
            assert_eq!(
                script_location("    at onRequest (eval_script:3:9)\n").as_deref(),
                Some("line 3, column 9")
            );
            assert_eq!(
                script_location("    at eval_script:12\n").as_deref(),
                Some("line 12")
            );
            assert_eq!(script_location("no location"), None);
        }

        #[test]
        fn header_object_grouping() {
            let h = Headers(vec![
                ("X-A".into(), "1".into()),
                ("Accept".into(), "a".into()),
                ("x-a".into(), "2".into()),
                ("x-a".into(), "3".into()),
            ]);
            let v = headers_to_json(&h);
            assert_eq!(
                v,
                serde_json::json!({"X-A": ["1", "2", "3"], "Accept": "a"})
            );
            let back = headers_from_json(
                &serde_json::json!({"X-A": ["1", 2], "Accept": null, "New": true}),
                &h,
                &v,
            )
            .unwrap();
            assert_eq!(
                back.0,
                vec![
                    ("X-A".to_string(), "1".to_string()),
                    ("X-A".to_string(), "2".to_string()),
                    ("New".to_string(), "true".to_string())
                ]
            );
            assert!(headers_from_json(&serde_json::json!({"bad name": "x"}), &h, &v).is_err());
            assert!(headers_from_json(&serde_json::json!("x"), &h, &v).is_err());
            assert_eq!(headers_from_json(&v, &h, &v).unwrap(), h);
        }
    }
}
