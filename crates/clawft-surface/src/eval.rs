//! Binding expression evaluator (ADR-016 §5).
//!
//! Evaluates an [`Expr`] against an [`OntologySnapshot`] and an
//! optional lambda-parameter binding. No side effects; no I/O; no
//! `unsafe`. Returns a [`Value`] that the composer converts to
//! strings/numbers/bools for the underlying canon primitives.

use serde_json::Value as Json;
use thiserror::Error;

use crate::parse::expr::{BinOp, Expr};
use crate::substrate::OntologySnapshot;
use crate::tree::{AttrValue, Binding};

/// The runtime-value type evaluator results are carried in.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    /// An opaque JSON object — we keep the `serde_json` value so field
    /// accesses can peek into nested structures without a bespoke map.
    Json(Json),
}

impl Value {
    pub fn from_json(v: &Json) -> Self {
        match v {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => n
                .as_i64()
                .map(Value::Int)
                .or_else(|| n.as_f64().map(Value::Num))
                .unwrap_or(Value::Null),
            Json::String(s) => Value::Str(s.clone()),
            Json::Array(arr) => Value::List(arr.iter().map(Value::from_json).collect()),
            Json::Object(_) => Value::Json(v.clone()),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            Value::Num(n) => Some(*n != 0.0),
            Value::Null => Some(false),
            Value::Str(s) => Some(!s.is_empty()),
            Value::List(xs) => Some(!xs.is_empty()),
            Value::Json(j) => Some(!j.is_null()),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Str(s) => s.parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Num(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Num(n) => {
                // Avoid exponential for whole numbers.
                if n.fract() == 0.0 && n.abs() < 1e16 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            Value::Str(s) => s.clone(),
            Value::List(xs) => format!("[{} items]", xs.len()),
            Value::Json(j) => j.to_string(),
        }
    }

    pub fn field(&self, name: &str) -> Value {
        match self {
            Value::Json(Json::Object(map)) => {
                map.get(name).map(Value::from_json).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        }
    }

    pub fn as_list(&self) -> Option<Vec<Value>> {
        match self {
            Value::List(xs) => Some(xs.clone()),
            Value::Json(Json::Array(arr)) => Some(arr.iter().map(Value::from_json).collect()),
            Value::Json(Json::Object(map)) => {
                Some(map.values().map(Value::from_json).collect::<Vec<_>>())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("unknown function `{0}`")]
    UnknownFunction(String),
    #[error("wrong arity for `{name}`: expected {expected}, got {got}")]
    WrongArity {
        name: String,
        expected: usize,
        got: usize,
    },
    #[error("unbound lambda parameter `{0}`")]
    UnboundLambdaParam(String),
    #[error("lambda expected where non-lambda argument was supplied")]
    ExpectedLambda,
}

/// Evaluate a [`Binding`] in the current ontology snapshot. Literals
/// short-circuit the expression evaluator.
pub fn eval_binding(b: &Binding, snap: &OntologySnapshot) -> Result<Value, EvalError> {
    match b {
        Binding::Literal(v) => Ok(attr_to_value(v)),
        Binding::Expr(e) => eval(e, snap, None),
    }
}

pub(crate) fn attr_to_value(v: &AttrValue) -> Value {
    match v {
        AttrValue::Bool(b) => Value::Bool(*b),
        AttrValue::Number(n) => Value::Num(*n),
        AttrValue::Int(i) => Value::Int(*i),
        AttrValue::Str(s) => Value::Str(s.clone()),
        AttrValue::Array(arr) => Value::List(arr.iter().map(attr_to_value).collect()),
    }
}

/// Lambda-parameter binding used when evaluating list-combinator
/// bodies. The binding is a single (name, value) pair because M1.5
/// forbids nested lambdas.
#[derive(Clone)]
pub struct LambdaBinding<'a> {
    name: &'a str,
    value: Value,
}

pub fn eval(
    e: &Expr,
    snap: &OntologySnapshot,
    bind: Option<&LambdaBinding<'_>>,
) -> Result<Value, EvalError> {
    match e {
        Expr::Literal(v) => Ok(attr_to_value(v)),
        Expr::Path(path) => Ok(snap
            .read(path)
            .map(|v| Value::from_json(&v))
            .unwrap_or(Value::Null)),
        Expr::Access(inner, field) => {
            // Special case: if `inner` is an unbound ident reference
            // via a lambda binding, e.g. `s.status`, we need to parse
            // that — but our AST never produces bare idents outside
            // Lambda bodies. The `s` in `s.status` was itself parsed
            // as an `Expr::Call`? No — our grammar rejects bare
            // idents. Look at where `s` appears: it can only appear
            // as the lambda parameter in `s -> body`. But inside
            // `body`, `s.status` is parsed as `Access(<??>, "status")`.
            // The parser emits `Call` only after a parenthesis. A
            // bare `s` however fails to parse today. We fix this by
            // allowing an ident-continuation primary that, *if a
            // lambda binding is active*, resolves to the bound value.
            //
            // In practice the parser produces `Access` only on top of
            // a real primary, and `s.status` is parsed as though `s`
            // were a function name missing its `()`. That's rejected
            // at parse-time in the current grammar.
            //
            // The M1.5 workaround: the parser treats a bare ident
            // followed by `.` *inside a lambda body* as a parameter
            // reference. We achieve that by rewriting the parser to
            // accept a `Var` primary — added below in a minimal patch
            // (see `parse::expr`). For now, honour the Var node here.
            let base = eval(inner, snap, bind)?;
            // WEFT-422: `.first` / `.last` shorthand on list-shaped
            // values is equivalent to the `first(xs)` / `last(xs)`
            // function-call form. Falls back to ordinary field access
            // for non-list bases (so `obj.first` on a struct with a
            // `first` member still works).
            if (field == "first" || field == "last")
                && let Some(items) = base.as_list()
            {
                if items.is_empty() {
                    return Ok(Value::Null);
                }
                return Ok(if field == "first" {
                    items.first().cloned().unwrap()
                } else {
                    items.last().cloned().unwrap()
                });
            }
            Ok(base.field(field))
        }
        Expr::Call(name, args) => eval_call(name, args, snap, bind),
        Expr::Lambda(_, _) => Err(EvalError::ExpectedLambda),
        Expr::Binop(op, l, r) => {
            let lv = eval(l, snap, bind)?;
            let rv = eval(r, snap, bind)?;
            eval_binop(*op, &lv, &rv)
        }
        Expr::Neg(inner) => {
            let v = eval(inner, snap, bind)?;
            match v {
                Value::Int(i) => Ok(Value::Int(-i)),
                Value::Num(n) => Ok(Value::Num(-n)),
                _ => Err(EvalError::TypeMismatch("neg on non-number".into())),
            }
        }
        Expr::Not(inner) => {
            let v = eval(inner, snap, bind)?;
            Ok(Value::Bool(!v.as_bool().unwrap_or(false)))
        }
        Expr::Var(name) => match bind {
            Some(b) if b.name == name => Ok(b.value.clone()),
            _ => Err(EvalError::UnboundLambdaParam(name.clone())),
        },
    }
}

fn eval_call(
    name: &str,
    args: &[Expr],
    snap: &OntologySnapshot,
    bind: Option<&LambdaBinding<'_>>,
) -> Result<Value, EvalError> {
    match name {
        "count" => {
            if args.len() != 2 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 2,
                    got: args.len(),
                });
            }
            let list = eval(&args[0], snap, bind)?;
            let list = list
                .as_list()
                .ok_or_else(|| EvalError::TypeMismatch("count() on non-list".into()))?;
            let (param, body) = expect_lambda(&args[1])?;
            let mut n: i64 = 0;
            for item in list {
                let b = LambdaBinding {
                    name: param,
                    value: item,
                };
                let v = eval(body, snap, Some(&b))?;
                if v.as_bool().unwrap_or(false) {
                    n += 1;
                }
            }
            Ok(Value::Int(n))
        }
        "sort" => {
            // WEFT-423: ordering combinator from ADR-016 §5.
            // Signature: `sort(list, key)` — `key` is a single-arg
            // lambda that returns the value to compare on. Stable
            // ascending sort. Numeric values compare numerically;
            // strings compare lexicographically; mixed / unorderable
            // values fall back to display-string comparison so the
            // sort is total (and total order is what the composer
            // needs for stable rendering — never panic on a row).
            if args.len() != 2 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 2,
                    got: args.len(),
                });
            }
            let list = eval(&args[0], snap, bind)?;
            let list = list
                .as_list()
                .ok_or_else(|| EvalError::TypeMismatch("sort() on non-list".into()))?;
            let (param, body) = expect_lambda(&args[1])?;
            // Pre-compute keys so the comparator never re-evaluates
            // the lambda (which would multiply expression cost by
            // O(n log n) and surface its errors at unpredictable
            // points in the sort).
            let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(list.len());
            for item in list {
                let b = LambdaBinding {
                    name: param,
                    value: item.clone(),
                };
                let key = eval(body, snap, Some(&b))?;
                keyed.push((key, item));
            }
            keyed.sort_by(|(a, _), (b, _)| cmp(a, b).unwrap_or(std::cmp::Ordering::Equal));
            Ok(Value::List(keyed.into_iter().map(|(_, v)| v).collect()))
        }
        "filter" => {
            if args.len() != 2 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 2,
                    got: args.len(),
                });
            }
            let list = eval(&args[0], snap, bind)?;
            let list = list
                .as_list()
                .ok_or_else(|| EvalError::TypeMismatch("filter() on non-list".into()))?;
            let (param, body) = expect_lambda(&args[1])?;
            let mut out = Vec::new();
            for item in list {
                let b = LambdaBinding {
                    name: param,
                    value: item.clone(),
                };
                let v = eval(body, snap, Some(&b))?;
                if v.as_bool().unwrap_or(false) {
                    out.push(item);
                }
            }
            Ok(Value::List(out))
        }
        "len" => {
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let v = eval(&args[0], snap, bind)?;
            let list = v
                .as_list()
                .ok_or_else(|| EvalError::TypeMismatch("len() on non-list".into()))?;
            Ok(Value::Int(list.len() as i64))
        }
        "first" | "last" => {
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let v = eval(&args[0], snap, bind)?;
            let list = v
                .as_list()
                .ok_or_else(|| EvalError::TypeMismatch(format!("{}() on non-list", name)))?;
            if list.is_empty() {
                return Ok(Value::Null);
            }
            Ok(if name == "first" {
                list.first().cloned().unwrap()
            } else {
                list.last().cloned().unwrap()
            })
        }
        "fmt_percent" | "fmt_pct" => {
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let v = eval(&args[0], snap, bind)?;
            let n = v
                .as_f64()
                .ok_or_else(|| EvalError::TypeMismatch("fmt_percent on non-number".into()))?;
            // Values in [0,1] are treated as fractions, >=1 as raw %.
            let pct = if n.abs() <= 1.0 { n * 100.0 } else { n };
            Ok(Value::Str(format!("{:.1}%", pct)))
        }
        "fmt_number" => {
            if args.len() != 2 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 2,
                    got: args.len(),
                });
            }
            let n = eval(&args[0], snap, bind)?
                .as_f64()
                .ok_or_else(|| EvalError::TypeMismatch("fmt_number on non-number".into()))?;
            let decimals = eval(&args[1], snap, bind)?
                .as_i64()
                .ok_or_else(|| EvalError::TypeMismatch("fmt_number decimals".into()))?
                .max(0) as usize;
            Ok(Value::Str(format!("{:.*}", decimals, n)))
        }
        "fmt_count" => {
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let n = eval(&args[0], snap, bind)?
                .as_i64()
                .ok_or_else(|| EvalError::TypeMismatch("fmt_count on non-integer".into()))?;
            Ok(Value::Str(n.to_string()))
        }
        "fmt_duration" => {
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let ms = eval(&args[0], snap, bind)?
                .as_f64()
                .ok_or_else(|| EvalError::TypeMismatch("fmt_duration on non-number".into()))?;
            Ok(Value::Str(fmt_duration(ms)))
        }
        "fmt_bytes" => {
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let n = eval(&args[0], snap, bind)?
                .as_f64()
                .ok_or_else(|| EvalError::TypeMismatch("fmt_bytes on non-number".into()))?;
            Ok(Value::Str(fmt_bytes(n)))
        }
        "exists" => {
            // Single arg: check path presence. Accepts a Path expr only.
            if args.len() != 1 {
                return Err(EvalError::WrongArity {
                    name: name.into(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let path = match &args[0] {
                Expr::Path(p) => p.as_str(),
                _ => {
                    return Err(EvalError::TypeMismatch(
                        "exists() expects a $path literal".into(),
                    ));
                }
            };
            Ok(Value::Bool(snap.read(path).is_some()))
        }
        other => Err(EvalError::UnknownFunction(other.into())),
    }
}

fn expect_lambda(e: &Expr) -> Result<(&str, &Expr), EvalError> {
    if let Expr::Lambda(name, body) = e {
        Ok((name.as_str(), body.as_ref()))
    } else {
        Err(EvalError::ExpectedLambda)
    }
}

fn eval_binop(op: BinOp, l: &Value, r: &Value) -> Result<Value, EvalError> {
    use BinOp::*;
    match op {
        Add => {
            if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
                if let (Value::Int(_), Value::Int(_)) = (l, r) {
                    return Ok(Value::Int(a as i64 + b as i64));
                }
                // string + anything / anything + string → string concat
                if let (Value::Str(s), _) | (_, Value::Str(s)) = (l, r) {
                    let _ = s;
                    return Ok(Value::Str(format!(
                        "{}{}",
                        l.to_display_string(),
                        r.to_display_string()
                    )));
                }
                return Ok(Value::Num(a + b));
            }
            // Treat as string concatenation when either side is a string.
            Ok(Value::Str(format!(
                "{}{}",
                l.to_display_string(),
                r.to_display_string()
            )))
        }
        Sub => Ok(num_op(l, r, |a, b| a - b)),
        Mul => Ok(num_op(l, r, |a, b| a * b)),
        Div => Ok(num_op(l, r, |a, b| if b == 0.0 { 0.0 } else { a / b })),
        Eq => Ok(Value::Bool(equal(l, r))),
        Neq => Ok(Value::Bool(!equal(l, r))),
        Lt => Ok(Value::Bool(cmp(l, r).map(|o| o.is_lt()).unwrap_or(false))),
        Lte => Ok(Value::Bool(cmp(l, r).map(|o| o.is_le()).unwrap_or(false))),
        Gt => Ok(Value::Bool(cmp(l, r).map(|o| o.is_gt()).unwrap_or(false))),
        Gte => Ok(Value::Bool(cmp(l, r).map(|o| o.is_ge()).unwrap_or(false))),
        And => Ok(Value::Bool(
            l.as_bool().unwrap_or(false) && r.as_bool().unwrap_or(false),
        )),
        Or => Ok(Value::Bool(
            l.as_bool().unwrap_or(false) || r.as_bool().unwrap_or(false),
        )),
    }
}

fn num_op(l: &Value, r: &Value, f: impl Fn(f64, f64) -> f64) -> Value {
    let (la, lb) = (l.as_f64().unwrap_or(0.0), r.as_f64().unwrap_or(0.0));
    if matches!((l, r), (Value::Int(_), Value::Int(_))) {
        Value::Int(f(la, lb) as i64)
    } else {
        Value::Num(f(la, lb))
    }
}

fn equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Null, Value::Null) => true,
        _ => match (l.as_f64(), r.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() < f64::EPSILON,
            _ => l.to_display_string() == r.to_display_string(),
        },
    }
}

fn cmp(l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
    if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
        return a.partial_cmp(&b);
    }
    Some(l.to_display_string().cmp(&r.to_display_string()))
}

fn fmt_duration(ms: f64) -> String {
    let total = (ms / 1000.0) as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}h{}m", h, m)
    } else if m > 0 {
        format!("{}m{}s", m, s)
    } else {
        format!("{}s", s)
    }
}

fn fmt_bytes(n: f64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n.abs();
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{:.1} {}", v, UNITS[u])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::expr::parse;
    use serde_json::json;

    fn snap_with(k: &str, v: Json) -> OntologySnapshot {
        let mut s = OntologySnapshot::empty();
        s.put(k, v);
        s
    }

    #[test]
    fn count_filter_services() {
        let snap = snap_with(
            "substrate/kernel/services",
            json!([
                {"name": "a", "status": "healthy"},
                {"name": "b", "status": "healthy"},
                {"name": "c", "status": "at_risk"},
            ]),
        );
        let e = parse("count($substrate/kernel/services, s -> s.status == \"healthy\")").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.as_i64(), Some(2));
    }

    #[test]
    fn field_access_on_path() {
        let snap = snap_with(
            "substrate/kernel/status",
            json!({"state": "healthy", "uptime_ms": 3_600_000_u64}),
        );
        let e = parse("$substrate/kernel/status.state").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.to_display_string(), "healthy");
    }

    #[test]
    fn fmt_percent_fraction() {
        let e = parse("fmt_percent(0.42)").unwrap();
        let snap = OntologySnapshot::empty();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.to_display_string(), "42.0%");
    }

    #[test]
    fn exists_present_and_absent() {
        let snap = snap_with("substrate/x", json!(1));
        let e1 = parse("exists($substrate/x)").unwrap();
        let e2 = parse("exists($substrate/y)").unwrap();
        assert_eq!(eval(&e1, &snap, None).unwrap().as_bool(), Some(true));
        assert_eq!(eval(&e2, &snap, None).unwrap().as_bool(), Some(false));
    }

    #[test]
    fn evaluates_subtraction_path_minus_int() {
        // Regression guard for the `x - 1` parser bug: once parsed
        // correctly, the evaluator must produce the arithmetic result.
        let snap = snap_with("a", json!(5));
        let e = parse("$a - 1").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.as_i64(), Some(4));
    }

    #[test]
    fn first_last_field_shorthand() {
        // WEFT-422: `.first` / `.last` on a list are equivalent to
        // first(xs) / last(xs).
        let snap = snap_with(
            "substrate/services",
            json!([
                {"name": "a"},
                {"name": "b"},
                {"name": "c"},
            ]),
        );
        let e = parse("$substrate/services.first").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.field("name").to_display_string(), "a");
        let e = parse("$substrate/services.last").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.field("name").to_display_string(), "c");
    }

    #[test]
    fn first_last_function_form_still_works() {
        // Regression: the function-call form must continue to work
        // after the shorthand lands.
        let snap = snap_with("substrate/xs", json!([10, 20, 30]));
        let e = parse("first($substrate/xs)").unwrap();
        assert_eq!(eval(&e, &snap, None).unwrap().as_i64(), Some(10));
        let e = parse("last($substrate/xs)").unwrap();
        assert_eq!(eval(&e, &snap, None).unwrap().as_i64(), Some(30));
    }

    #[test]
    fn first_last_empty_list_is_null() {
        let snap = snap_with("substrate/xs", json!([]));
        let e = parse("$substrate/xs.first").unwrap();
        assert!(matches!(eval(&e, &snap, None).unwrap(), Value::Null));
    }

    #[test]
    fn sort_by_field_ascending() {
        // WEFT-423: `sort(list, key)` orders by the key lambda's
        // returned value.
        let snap = snap_with(
            "substrate/services",
            json!([
                {"name": "c", "weight": 3},
                {"name": "a", "weight": 1},
                {"name": "b", "weight": 2},
            ]),
        );
        let e = parse("sort($substrate/services, s -> s.weight)").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        let xs = v.as_list().unwrap();
        assert_eq!(xs.len(), 3);
        assert_eq!(xs[0].field("name").to_display_string(), "a");
        assert_eq!(xs[1].field("name").to_display_string(), "b");
        assert_eq!(xs[2].field("name").to_display_string(), "c");
    }

    #[test]
    fn sort_by_derived_value() {
        // Sort by a computed expression (negation flips order).
        let snap = snap_with("substrate/xs", json!([1, 3, 2]));
        let e = parse("sort($substrate/xs, x -> -x)").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        let xs = v.as_list().unwrap();
        assert_eq!(xs.len(), 3);
        assert_eq!(xs[0].as_i64(), Some(3));
        assert_eq!(xs[1].as_i64(), Some(2));
        assert_eq!(xs[2].as_i64(), Some(1));
    }

    #[test]
    fn evaluates_hex_and_scientific_literals() {
        // WEFT-424: literal-form coverage at eval time.
        let snap = OntologySnapshot::empty();
        let e = parse("0xff").unwrap();
        assert_eq!(eval(&e, &snap, None).unwrap().as_i64(), Some(255));
        let e = parse("1e3 + 5").unwrap();
        assert_eq!(eval(&e, &snap, None).unwrap().as_f64(), Some(1005.0));
    }

    #[test]
    fn evaluates_chained_binops_left_assoc() {
        // `$a - $b + $c` with {a:10, b:3, c:2} must yield 9 = (10-3)+2.
        let mut snap = OntologySnapshot::empty();
        snap.put("a", json!(10));
        snap.put("b", json!(3));
        snap.put("c", json!(2));
        let e = parse("$a - $b + $c").unwrap();
        let v = eval(&e, &snap, None).unwrap();
        assert_eq!(v.as_i64(), Some(9));
    }
}
