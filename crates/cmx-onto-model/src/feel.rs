//! FEEL 表达式子集引擎（O4-M2 动作校验用；离线自研，clean-room 对齐 cmx-rulesengine::feel::expr）。
//!
//! 自持 tokenizer + Pratt 解析器 + 求值器 + 内置函数库。值模型用 `serde_json::Value`，数值 f64。
//! 独立微服务纪律：本体不跨 workspace 依赖规则引擎，故把判定所需的 FEEL 子集内嵌于内核（零 IO、可单测）。
//!
//! 支持：字面量、变量与路径（`order.amount`）、算术（`+ - * / **`、一元 `-`）、比较、逻辑
//! （`and or not`）、条件（`if…then…else`）、区间与成员（`x in [1..10]` / `x in list`）、
//! 函数调用（内置库）、列表推导与量词（`for/some/every`、过滤 `l[cond]`）。仅暴露 [`eval_expression`]。

use serde_json::{json, Map, Value};

/// FEEL 求值错误。
#[derive(Debug, thiserror::Error)]
pub enum FeelError {
    #[error("FEEL 语法错误: {0}")]
    Syntax(String),
}
type Result<T> = std::result::Result<T, FeelError>;

/// 求值一个 FEEL 表达式，`ctx` 为输入事实（JSON 对象，其字段即顶层变量）。
pub fn eval_expression(src: &str, ctx: &Value) -> Result<Value> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks, pos: 0 };
    let ast = p.parse_expr(0)?;
    if p.peek().is_some() {
        return Err(FeelError::Syntax(format!("表达式尾部有多余记号: {src:?}")));
    }
    let scope = Scope::root(ctx);
    eval(&ast, &scope)
}

/// 求值为布尔判定（校验用）：非布尔结果视为不满足（fail-closed），null/错误交调用方处理。
pub fn eval_predicate(src: &str, ctx: &Value) -> Result<bool> {
    Ok(matches!(eval_expression(src, ctx)?, Value::Bool(true)))
}

// ═══════════════════════════ Tokenizer ═══════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Kw(&'static str),
    LParen,
    RParen,
    LBrack,
    RBrack,
    Comma,
    Dot,
    DotDot,
    Plus,
    Minus,
    Star,
    Slash,
    StarStar,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

const KEYWORDS: &[&str] = &[
    "and", "or", "not", "true", "false", "null", "if", "then", "else", "for", "in", "return",
    "some", "every", "satisfies",
];

fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let n = b.len();
    let mut out = Vec::new();
    while i < n {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => { out.push(Tok::LParen); i += 1; }
            ')' => { out.push(Tok::RParen); i += 1; }
            '[' => { out.push(Tok::LBrack); i += 1; }
            ']' => { out.push(Tok::RBrack); i += 1; }
            ',' => { out.push(Tok::Comma); i += 1; }
            '+' => { out.push(Tok::Plus); i += 1; }
            '-' => { out.push(Tok::Minus); i += 1; }
            '/' => { out.push(Tok::Slash); i += 1; }
            '*' => {
                if i + 1 < n && b[i + 1] == '*' { out.push(Tok::StarStar); i += 2; }
                else { out.push(Tok::Star); i += 1; }
            }
            '<' => {
                if i + 1 < n && b[i + 1] == '=' { out.push(Tok::Le); i += 2; }
                else { out.push(Tok::Lt); i += 1; }
            }
            '>' => {
                if i + 1 < n && b[i + 1] == '=' { out.push(Tok::Ge); i += 2; }
                else { out.push(Tok::Gt); i += 1; }
            }
            '=' => { out.push(Tok::Eq); i += 1; }
            '!' => {
                if i + 1 < n && b[i + 1] == '=' { out.push(Tok::Ne); i += 2; }
                else { return Err(FeelError::Syntax("非法记号 '!'（应为 !=）".into())); }
            }
            '.' => {
                if i + 1 < n && b[i + 1] == '.' { out.push(Tok::DotDot); i += 2; }
                else { out.push(Tok::Dot); i += 1; }
            }
            '"' | '\'' => {
                let quote = c;
                i += 1;
                let mut s = String::new();
                while i < n && b[i] != quote {
                    if b[i] == '\\' && i + 1 < n {
                        i += 1;
                        s.push(match b[i] { 'n' => '\n', 't' => '\t', other => other });
                    } else {
                        s.push(b[i]);
                    }
                    i += 1;
                }
                if i >= n { return Err(FeelError::Syntax("字符串未闭合".into())); }
                i += 1;
                out.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < n && b[i].is_ascii_digit() { i += 1; }
                if i + 1 < n && b[i] == '.' && b[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < n && b[i].is_ascii_digit() { i += 1; }
                }
                let lit: String = b[start..i].iter().collect();
                let v: f64 = lit.parse().map_err(|_| FeelError::Syntax(format!("非法数字: {lit}")))?;
                out.push(Tok::Num(v));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (b[i].is_alphanumeric() || b[i] == '_') { i += 1; }
                let word: String = b[start..i].iter().collect();
                match KEYWORDS.iter().find(|k| **k == word) {
                    Some(kw) => out.push(Tok::Kw(kw)),
                    None => out.push(Tok::Ident(word)),
                }
            }
            other => return Err(FeelError::Syntax(format!("非法字符: {other:?}"))),
        }
    }
    Ok(out)
}

// ═══════════════════════════ AST + Parser ═══════════════════════════

#[derive(Debug, Clone)]
enum Ast {
    Lit(Value),
    Var(String),
    List(Vec<Ast>),
    Member(Box<Ast>, String),
    Index(Box<Ast>, Box<Ast>),
    Interval { lo: Box<Ast>, hi: Box<Ast>, lo_incl: bool, hi_incl: bool },
    Neg(Box<Ast>),
    Not(Box<Ast>),
    Bin(BinOp, Box<Ast>, Box<Ast>),
    If(Box<Ast>, Box<Ast>, Box<Ast>),
    For(String, Box<Ast>, Box<Ast>),
    Quant { every: bool, var: String, list: Box<Ast>, cond: Box<Ast> },
    Call(String, Vec<Ast>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BinOp { Add, Sub, Mul, Div, Pow, Lt, Le, Gt, Ge, Eq, Ne, And, Or, In }

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    fn next(&mut self) -> Option<Tok> { let t = self.toks.get(self.pos).cloned(); if t.is_some() { self.pos += 1; } t }
    fn eat(&mut self, t: &Tok) -> Result<()> {
        if self.peek() == Some(t) { self.pos += 1; Ok(()) }
        else { Err(FeelError::Syntax(format!("期望 {t:?}，实为 {:?}", self.peek()))) }
    }
    fn eat_kw(&mut self, kw: &str) -> Result<()> {
        if self.peek() == Some(&Tok::Kw(kw_static(kw))) { self.pos += 1; Ok(()) }
        else { Err(FeelError::Syntax(format!("期望关键字 {kw}，实为 {:?}", self.peek()))) }
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Ast> {
        let mut lhs = self.parse_prefix()?;
        loop {
            let (op, lbp, rbp) = match self.peek() {
                Some(Tok::Kw("or")) => (BinOp::Or, 1, 2),
                Some(Tok::Kw("and")) => (BinOp::And, 3, 4),
                Some(Tok::Lt) => (BinOp::Lt, 5, 6),
                Some(Tok::Le) => (BinOp::Le, 5, 6),
                Some(Tok::Gt) => (BinOp::Gt, 5, 6),
                Some(Tok::Ge) => (BinOp::Ge, 5, 6),
                Some(Tok::Eq) => (BinOp::Eq, 5, 6),
                Some(Tok::Ne) => (BinOp::Ne, 5, 6),
                Some(Tok::Kw("in")) => (BinOp::In, 5, 6),
                Some(Tok::Plus) => (BinOp::Add, 7, 8),
                Some(Tok::Minus) => (BinOp::Sub, 7, 8),
                Some(Tok::Star) => (BinOp::Mul, 9, 10),
                Some(Tok::Slash) => (BinOp::Div, 9, 10),
                Some(Tok::StarStar) => (BinOp::Pow, 12, 11),
                _ => break,
            };
            if lbp < min_bp { break; }
            self.pos += 1;
            let rhs = self.parse_expr(rbp)?;
            lhs = Ast::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Ast> {
        match self.peek().cloned() {
            Some(Tok::Minus) => { self.pos += 1; Ok(Ast::Neg(Box::new(self.parse_expr(11)?))) }
            Some(Tok::Kw("not")) => {
                self.pos += 1;
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let inner = self.parse_expr(0)?;
                    self.eat(&Tok::RParen)?;
                    Ok(Ast::Not(Box::new(inner)))
                } else {
                    Ok(Ast::Not(Box::new(self.parse_expr(6)?)))
                }
            }
            Some(Tok::Kw("if")) => {
                self.pos += 1;
                let c = self.parse_expr(0)?;
                self.eat_kw("then")?;
                let a = self.parse_expr(0)?;
                self.eat_kw("else")?;
                let b = self.parse_expr(0)?;
                Ok(Ast::If(Box::new(c), Box::new(a), Box::new(b)))
            }
            Some(Tok::Kw("for")) => {
                self.pos += 1;
                let var = self.ident()?;
                self.eat_kw("in")?;
                let list = self.parse_expr(0)?;
                self.eat_kw("return")?;
                let body = self.parse_expr(0)?;
                Ok(Ast::For(var, Box::new(list), Box::new(body)))
            }
            Some(Tok::Kw("some")) | Some(Tok::Kw("every")) => {
                let every = self.peek() == Some(&Tok::Kw("every"));
                self.pos += 1;
                let var = self.ident()?;
                self.eat_kw("in")?;
                let list = self.parse_expr(0)?;
                self.eat_kw("satisfies")?;
                let cond = self.parse_expr(0)?;
                Ok(Ast::Quant { every, var, list: Box::new(list), cond: Box::new(cond) })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Ast> {
        let mut node = self.parse_primary()?;
        loop {
            match self.peek() {
                Some(Tok::Dot) => {
                    self.pos += 1;
                    let field = self.ident()?;
                    node = Ast::Member(Box::new(node), field);
                }
                Some(Tok::LBrack) => {
                    self.pos += 1;
                    let idx = self.parse_expr(0)?;
                    self.eat(&Tok::RBrack)?;
                    node = Ast::Index(Box::new(node), Box::new(idx));
                }
                _ => break,
            }
        }
        Ok(node)
    }

    fn parse_primary(&mut self) -> Result<Ast> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Ast::Lit(json!(n))),
            Some(Tok::Str(s)) => Ok(Ast::Lit(Value::String(s))),
            Some(Tok::Kw("true")) => Ok(Ast::Lit(json!(true))),
            Some(Tok::Kw("false")) => Ok(Ast::Lit(json!(false))),
            Some(Tok::Kw("null")) => Ok(Ast::Lit(Value::Null)),
            Some(Tok::Ident(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr(0)?);
                            if self.peek() == Some(&Tok::Comma) { self.pos += 1; } else { break; }
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    Ok(Ast::Call(name, args))
                } else {
                    Ok(Ast::Var(name))
                }
            }
            Some(Tok::LBrack) => {
                let first = self.parse_expr(0)?;
                if self.peek() == Some(&Tok::DotDot) {
                    self.pos += 1;
                    let hi = self.parse_expr(0)?;
                    let hi_incl = match self.next() {
                        Some(Tok::RBrack) => true,
                        Some(Tok::RParen) => false,
                        other => return Err(FeelError::Syntax(format!("区间上界应以 ] 或 ) 结束，实为 {other:?}"))),
                    };
                    Ok(Ast::Interval { lo: Box::new(first), hi: Box::new(hi), lo_incl: true, hi_incl })
                } else {
                    let mut items = vec![first];
                    while self.peek() == Some(&Tok::Comma) { self.pos += 1; items.push(self.parse_expr(0)?); }
                    self.eat(&Tok::RBrack)?;
                    Ok(Ast::List(items))
                }
            }
            Some(Tok::LParen) => {
                let first = self.parse_expr(0)?;
                if self.peek() == Some(&Tok::DotDot) {
                    self.pos += 1;
                    let hi = self.parse_expr(0)?;
                    let hi_incl = match self.next() {
                        Some(Tok::RBrack) => true,
                        Some(Tok::RParen) => false,
                        other => return Err(FeelError::Syntax(format!("区间上界应以 ] 或 ) 结束，实为 {other:?}"))),
                    };
                    Ok(Ast::Interval { lo: Box::new(first), hi: Box::new(hi), lo_incl: false, hi_incl })
                } else {
                    self.eat(&Tok::RParen)?;
                    Ok(first)
                }
            }
            other => Err(FeelError::Syntax(format!("非预期记号: {other:?}"))),
        }
    }

    fn ident(&mut self) -> Result<String> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            other => Err(FeelError::Syntax(format!("期望标识符，实为 {other:?}"))),
        }
    }
}

fn kw_static(kw: &str) -> &'static str {
    KEYWORDS.iter().find(|k| **k == kw).copied().unwrap_or("")
}

// ═══════════════════════════ Evaluator ═══════════════════════════

struct Scope<'a> {
    vars: Map<String, Value>,
    parent: Option<&'a Scope<'a>>,
}

impl<'a> Scope<'a> {
    fn root(ctx: &Value) -> Scope<'static> {
        let vars = match ctx {
            Value::Object(m) => m.clone(),
            _ => Map::new(),
        };
        Scope { vars, parent: None }
    }
    fn child(&'a self, name: String, val: Value) -> Scope<'a> {
        let mut vars = Map::new();
        if let Value::Object(m) = &val {
            for (k, v) in m { vars.insert(k.clone(), v.clone()); }
        }
        vars.insert(name, val);
        Scope { vars, parent: Some(self) }
    }
    fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) { return Some(v.clone()); }
        self.parent.and_then(|p| p.get(name))
    }
}

fn eval(ast: &Ast, scope: &Scope) -> Result<Value> {
    match ast {
        Ast::Lit(v) => Ok(v.clone()),
        Ast::Var(name) => Ok(scope.get(name).unwrap_or(Value::Null)),
        Ast::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items { out.push(eval(it, scope)?); }
            Ok(Value::Array(out))
        }
        Ast::Member(base, field) => {
            let b = eval(base, scope)?;
            Ok(match b { Value::Object(m) => m.get(field).cloned().unwrap_or(Value::Null), _ => Value::Null })
        }
        Ast::Index(base, idx) => {
            let b = eval(base, scope)?;
            let Value::Array(arr) = b else { return Ok(Value::Null); };
            let iv = eval(idx, scope)?;
            if let Some(n) = iv.as_f64() {
                let len = arr.len() as i64;
                let i = n as i64;
                let real = if i < 0 { len + i } else { i - 1 };
                if real >= 0 && real < len { Ok(arr[real as usize].clone()) } else { Ok(Value::Null) }
            } else {
                let mut out = Vec::new();
                for el in &arr {
                    let child = scope.child("item".into(), el.clone());
                    if truthy(&eval(idx, &child)?) { out.push(el.clone()); }
                }
                Ok(Value::Array(out))
            }
        }
        Ast::Interval { .. } => Err(FeelError::Syntax("区间只能用于 `x in [a..b]` 的右侧".into())),
        Ast::Neg(e) => {
            let v = eval(e, scope)?;
            Ok(v.as_f64().map(|n| json!(-n)).unwrap_or(Value::Null))
        }
        Ast::Not(e) => Ok(json!(!truthy(&eval(e, scope)?))),
        Ast::If(c, a, b) => if truthy(&eval(c, scope)?) { eval(a, scope) } else { eval(b, scope) },
        Ast::For(var, list, body) => {
            let lv = eval(list, scope)?;
            let items = as_array(&lv);
            let mut out = Vec::new();
            for el in items { let child = scope.child(var.clone(), el.clone()); out.push(eval(body, &child)?); }
            Ok(Value::Array(out))
        }
        Ast::Quant { every, var, list, cond } => {
            let lv = eval(list, scope)?;
            let items = as_array(&lv);
            let mut acc = *every;
            for el in items {
                let child = scope.child(var.clone(), el.clone());
                let t = truthy(&eval(cond, &child)?);
                if *every { acc = acc && t; if !acc { break; } } else { acc = acc || t; if acc { break; } }
            }
            Ok(json!(acc))
        }
        Ast::Bin(op, l, r) => eval_bin(*op, l, r, scope),
        Ast::Call(name, args) => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args { vals.push(eval(a, scope)?); }
            call_builtin(name, &vals)
        }
    }
}

fn eval_bin(op: BinOp, l: &Ast, r: &Ast, scope: &Scope) -> Result<Value> {
    if op == BinOp::And { return Ok(json!(truthy(&eval(l, scope)?) && truthy(&eval(r, scope)?))); }
    if op == BinOp::Or { return Ok(json!(truthy(&eval(l, scope)?) || truthy(&eval(r, scope)?))); }
    if op == BinOp::In {
        let lv = eval(l, scope)?;
        return eval_in(&lv, r, scope);
    }
    let a = eval(l, scope)?;
    let b = eval(r, scope)?;
    match op {
        BinOp::Add => match (&a, &b) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::String(x), _) => Ok(Value::String(format!("{x}{}", to_str(&b)))),
            (_, Value::String(y)) => Ok(Value::String(format!("{}{y}", to_str(&a)))),
            _ => num2(&a, &b, |x, y| x + y),
        },
        BinOp::Sub => num2(&a, &b, |x, y| x - y),
        BinOp::Mul => num2(&a, &b, |x, y| x * y),
        BinOp::Div => match (a.as_f64(), b.as_f64()) {
            (Some(_), Some(0.0)) => Ok(Value::Null),
            (Some(x), Some(y)) => Ok(json!(x / y)),
            _ => Ok(Value::Null),
        },
        BinOp::Pow => num2(&a, &b, |x, y| x.powf(y)),
        BinOp::Eq => Ok(json!(feel_eq(&a, &b))),
        BinOp::Ne => Ok(json!(!feel_eq(&a, &b))),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Ok(json!(cmp(op, &a, &b))),
        _ => unreachable!(),
    }
}

fn eval_in(lv: &Value, r: &Ast, scope: &Scope) -> Result<Value> {
    if let Ast::Interval { lo, hi, lo_incl, hi_incl } = r {
        let (lo, hi) = (eval(lo, scope)?, eval(hi, scope)?);
        let (Some(x), Some(a), Some(b)) = (lv.as_f64(), lo.as_f64(), hi.as_f64()) else { return Ok(json!(false)); };
        let lo_ok = if *lo_incl { x >= a } else { x > a };
        let hi_ok = if *hi_incl { x <= b } else { x < b };
        return Ok(json!(lo_ok && hi_ok));
    }
    let rv = eval(r, scope)?;
    Ok(match rv {
        Value::Array(arr) => json!(arr.iter().any(|e| feel_eq(e, lv))),
        other => json!(feel_eq(&other, lv)),
    })
}

fn truthy(v: &Value) -> bool { matches!(v, Value::Bool(true)) }
fn as_array(v: &Value) -> Vec<Value> { match v { Value::Array(a) => a.clone(), Value::Null => vec![], other => vec![other.clone()] } }
fn feel_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}
fn cmp(op: BinOp, a: &Value, b: &Value) -> bool {
    let ord = match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.partial_cmp(&y),
        _ => match (a, b) { (Value::String(x), Value::String(y)) => Some(x.cmp(y)), _ => None },
    };
    let Some(o) = ord else { return false };
    use std::cmp::Ordering::*;
    match op {
        BinOp::Lt => o == Less,
        BinOp::Le => o != Greater,
        BinOp::Gt => o == Greater,
        BinOp::Ge => o != Less,
        _ => false,
    }
}
fn num2(a: &Value, b: &Value, f: impl Fn(f64, f64) -> f64) -> Result<Value> {
    match (a.as_f64(), b.as_f64()) { (Some(x), Some(y)) => Ok(json!(f(x, y))), _ => Ok(Value::Null) }
}
fn to_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Number(n) => n.as_f64().map(feel_num_str).unwrap_or_else(|| n.to_string()),
        other => other.to_string(),
    }
}
fn feel_num_str(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        format!("{}", x as i64)
    } else {
        x.to_string()
    }
}

fn call_builtin(name: &str, a: &[Value]) -> Result<Value> {
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Null);
    let numf = |i: usize| a.get(i).and_then(|v| v.as_f64());
    let strf = |i: usize| a.get(i).and_then(|v| v.as_str().map(str::to_string));
    let list_of = |v: &Value| -> Vec<Value> { as_array(v) };
    let nums = |v: &Value| -> Vec<f64> { as_array(v).iter().filter_map(|x| x.as_f64()).collect() };
    Ok(match name {
        "floor" => numf(0).map(|x| json!(x.floor())).unwrap_or(Value::Null),
        "ceiling" | "ceil" => numf(0).map(|x| json!(x.ceil())).unwrap_or(Value::Null),
        "round" => {
            let d = numf(1).unwrap_or(0.0) as i32;
            numf(0).map(|x| { let p = 10f64.powi(d); json!((x * p).round() / p) }).unwrap_or(Value::Null)
        }
        "abs" => numf(0).map(|x| json!(x.abs())).unwrap_or(Value::Null),
        "modulo" => match (numf(0), numf(1)) { (Some(x), Some(y)) if y != 0.0 => json!(x - y * (x / y).floor()), _ => Value::Null },
        "sqrt" => numf(0).filter(|x| *x >= 0.0).map(|x| json!(x.sqrt())).unwrap_or(Value::Null),
        "min" => { let v = collect_nums(a, &nums); v.iter().cloned().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x)))).map(|x| json!(x)).unwrap_or(Value::Null) }
        "max" => { let v = collect_nums(a, &nums); v.iter().cloned().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))).map(|x| json!(x)).unwrap_or(Value::Null) }
        "sum" => json!(collect_nums(a, &nums).iter().sum::<f64>()),
        "mean" | "avg" => { let v = collect_nums(a, &nums); if v.is_empty() { Value::Null } else { json!(v.iter().sum::<f64>() / v.len() as f64) } }
        "upper" | "upperCase" => strf(0).map(|s| json!(s.to_uppercase())).unwrap_or(Value::Null),
        "lower" | "lowerCase" => strf(0).map(|s| json!(s.to_lowercase())).unwrap_or(Value::Null),
        "substring" => {
            let s = strf(0).unwrap_or_default();
            let chars: Vec<char> = s.chars().collect();
            let start = numf(1).unwrap_or(1.0) as i64;
            let st = if start < 0 { (chars.len() as i64 + start).max(0) } else { (start - 1).max(0) } as usize;
            let len = numf(2).map(|l| l as usize).unwrap_or(chars.len().saturating_sub(st));
            json!(chars.iter().skip(st).take(len).collect::<String>())
        }
        "contains" => {
            match arg(0) {
                Value::String(s) => json!(s.contains(&strf(1).unwrap_or_default())),
                Value::Array(arr) => json!(arr.iter().any(|e| feel_eq(e, &arg(1)))),
                _ => json!(false),
            }
        }
        "startsWith" | "startswith" => json!(strf(0).unwrap_or_default().starts_with(&strf(1).unwrap_or_default())),
        "endsWith" | "endswith" => json!(strf(0).unwrap_or_default().ends_with(&strf(1).unwrap_or_default())),
        "concatenate" | "concat" => json!(a.iter().map(to_str).collect::<String>()),
        "string" => json!(to_str(&arg(0))),
        "number" => json!(strf(0).and_then(|s| s.parse::<f64>().ok())),
        "trim" => strf(0).map(|s| json!(s.trim())).unwrap_or(Value::Null),
        "count" | "length" | "len" => match arg(0) {
            Value::Array(arr) => json!(arr.len()),
            Value::String(s) => json!(s.chars().count()),
            _ => json!(0),
        },
        "sort" => { let mut v = list_of(&arg(0)); v.sort_by(|x, y| x.as_f64().partial_cmp(&y.as_f64()).unwrap_or(std::cmp::Ordering::Equal)); Value::Array(v) }
        "append" => { let mut v = list_of(&arg(0)); v.extend(a.iter().skip(1).cloned()); Value::Array(v) }
        "not" => json!(!truthy(&arg(0))),
        "coalesce" => a.iter().find(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
        _ => return Err(FeelError::Syntax(format!("未知函数: {name}"))),
    })
}

fn collect_nums(a: &[Value], nums: &dyn Fn(&Value) -> Vec<f64>) -> Vec<f64> {
    if a.len() == 1 { nums(&a[0]) } else { a.iter().filter_map(|v| v.as_f64()).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(src: &str, ctx: Value) -> Value {
        eval_expression(src, &ctx).unwrap()
    }

    #[test]
    fn arithmetic_and_compare() {
        assert_eq!(ev("amount > 0", json!({"amount": 5})), json!(true));
        assert_eq!(ev("amount > 0", json!({"amount": -1})), json!(false));
        assert_eq!(ev("a + b * 2", json!({"a": 1, "b": 3})), json!(7.0));
    }

    #[test]
    fn logic_and_membership() {
        assert_eq!(ev("status = 'open' and amount >= 100", json!({"status": "open", "amount": 100})), json!(true));
        assert_eq!(ev("status in ['open','pending']", json!({"status": "pending"})), json!(true));
        assert_eq!(ev("x in [1..10]", json!({"x": 5})), json!(true));
        assert_eq!(ev("x in [1..10]", json!({"x": 11})), json!(false));
    }

    #[test]
    fn path_and_builtins() {
        assert_eq!(ev("order.amount >= 50", json!({"order": {"amount": 60}})), json!(true));
        assert_eq!(ev("upper(name) = 'ADA'", json!({"name": "ada"})), json!(true));
        assert_eq!(ev("count(items) = 3", json!({"items": [1,2,3]})), json!(true));
    }

    #[test]
    fn predicate_helper_fail_closed() {
        // 非布尔结果 → 判定 false（fail-closed）
        assert_eq!(eval_predicate("amount", &json!({"amount": 5})).unwrap(), false);
        assert_eq!(eval_predicate("amount > 0", &json!({"amount": 5})).unwrap(), true);
    }

    #[test]
    fn syntax_error_surfaces() {
        assert!(eval_expression("amount >", &json!({})).is_err());
    }
}
