//! The formula language: spreadsheet-like calculations people write in the app, compiled to SQL.
//!
//! A formula is parsed by this module into a tree, and SQL is built only from that tree: the
//! model's fields (resolved by the caller, so only what the model defines), numbers that parse as
//! finite numbers, and a fixed set of operators and functions. Nothing a person types reaches the
//! SQL as text, so formulas keep the "no SQL from users" rule.
//!
//! Step 1 (measure formulas): calculations on totals, such as `(revenue - cost) / revenue` or
//! `sum(amount) / count_distinct(store)`, compiled into the query where the model's measures are.

use crate::compiler::Dialect;

/// Longer formulas are refused: they're written by people, and this bounds the work they cause.
const MAX_LENGTH: usize = 1000;
/// How deep brackets, functions and signs may nest (the parser recurses on them).
const MAX_DEPTH: usize = 32;
/// The functions a formula may call, besides aggregations.
#[rustfmt::skip]
pub const FUNCTIONS: [&str; 57] = [
    "if", "coalesce", "round", "abs", "least", "greatest", "in", "case", // numbers, logic
    "floor", "ceil", "mod", "power", "sqrt", "ln", "log10", "exp",
    "lower", "upper", "concat", "contains", "starts_with", "ends_with", "length", "trim", "left", "right", "replace",
    "substring", "position", "split_part", "to_number", "to_text",
    "year", "quarter", "month", "week", "day", "weekday", "hour", "today", "days_between", // dates
    "date_trunc", "date_add", "date_diff", "hours_between", "minutes_between", "format_date", "fiscal_year", "fiscal_quarter",
    "sum_if", "avg_if", "count_if", "min_if", "max_if", "count_distinct_if", "stddev", "variance", // over the rows
];
/// On totals, the functions that add up rows (each row's columns inside them).
pub const OVER_ROWS: [&str; 8] =
    ["sum_if", "avg_if", "count_if", "min_if", "max_if", "count_distinct_if", "stddev", "variance"];
/// The parts of a date the functions of that name give, as numbers.
const DATE_PARTS: [&str; 7] = ["year", "quarter", "month", "week", "day", "weekday", "hour"];
/// Calculations on a chart's results (window functions, run on ScramDB: results.rs).
#[rustfmt::skip]
pub const RESULT_FUNCTIONS: [&str; 13] = [
    "running_sum", "moving_avg", "previous", "rank", "percent_of_total",
    "running_avg", "next", "difference", "percent_change", "dense_rank", "first", "last", "ntile",
];
/// Aggregations of a column, as in Explore's Σ: `sum(amount)`.
pub const AGGREGATIONS: [&str; 6] = ["sum", "avg", "min", "max", "count", "count_distinct"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaError {
    /// Where in the formula (a byte offset; formulas are ASCII)
    pub at: usize,
    pub message: String,
}

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at character {})", self.message, self.at + 1)
    }
}

fn fail<T>(at: usize, message: impl Into<String>) -> Result<T, FormulaError> {
    Err(FormulaError { at, message: message.into() })
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(String),
    /// 'text', quotes doubled inside
    Text(String),
    Name(String),
    Symbol(&'static str),
    End,
}

fn tokens(expr: &str) -> Result<Vec<(usize, Token)>, FormulaError> {
    let b = expr.as_bytes();
    let (mut out, mut i, mut depth) = (Vec::new(), 0, 0usize);
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text = &expr[start..i];
            match text.parse::<f64>() {
                Ok(n) if n.is_finite() => out.push((start, Token::Number(text.to_string()))),
                _ => return fail(start, format!("{text:?} isn't a number this can use")),
            }
            continue;
        }
        if c == b'\'' {
            let mut text = String::new();
            loop {
                i += 1;
                match expr[i..].find('\'') {
                    None => return fail(start, "this text is missing its closing '"),
                    Some(j) => {
                        text.push_str(&expr[i..i + j]);
                        i += j + 1;
                        if b.get(i) != Some(&b'\'') {
                            break;
                        }
                        text.push('\'');
                    }
                }
            }
            out.push((start, Token::Text(text)));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let word = |i: &mut usize| {
                while *i < b.len() && (b[*i].is_ascii_alphanumeric() || b[*i] == b'_') {
                    *i += 1;
                }
            };
            word(&mut i);
            // a blend's field: model.field (one dot, between two names)
            if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') {
                i += 1;
                word(&mut i);
                if b.get(i) == Some(&b'.') {
                    return fail(i, "a field is a name, or model.field: one dot");
                }
            }
            out.push((start, Token::Name(expr[start..i].to_string())));
            continue;
        }
        let two = expr.get(i..i + 2).unwrap_or("");
        let symbol = match two {
            "<=" | ">=" | "<>" | "!=" => {
                i += 2;
                match two {
                    "<=" => "<=",
                    ">=" => ">=",
                    _ => "<>",
                }
            }
            _ => {
                i += 1;
                match c {
                    b'+' => "+",
                    b'-' => "-",
                    b'*' => "*",
                    b'/' => "/",
                    b'=' => "=",
                    b'<' => "<",
                    b'>' => ">",
                    b',' => ",",
                    b'(' => "(",
                    b')' => ")",
                    _ => {
                        let shown = expr[start..].chars().next().unwrap_or(' ');
                        return fail(start, format!("{shown:?} can't be used in a formula"));
                    }
                }
            }
        };
        if symbol == "(" {
            depth += 1;
            if depth > MAX_DEPTH {
                return fail(start, "this formula is nested too deeply");
            }
        } else if symbol == ")" {
            depth = depth.saturating_sub(1);
        }
        out.push((start, Token::Symbol(symbol)));
    }
    if expr.len() > MAX_LENGTH {
        return fail(MAX_LENGTH, format!("this formula is too long (at most {MAX_LENGTH} characters)"));
    }
    out.push((expr.len(), Token::End));
    Ok(out)
}

/// A parsed formula. Fields are kept by name; the caller says what they mean.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Number(String),
    Text(String),
    Bool(bool),
    Null,
    /// A model field (a measure, or another formula), or an aggregation such as `sum(amount)`
    Field {
        name: String,
        at: usize,
    },
    Neg(Box<Node>),
    Not(Box<Node>),
    Binary {
        op: &'static str,
        left: Box<Node>,
        right: Box<Node>,
    },
    Call {
        name: String,
        args: Vec<Node>,
        at: usize,
    },
}

struct Parser {
    tokens: Vec<(usize, Token)>,
    next: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.next].1
    }

    fn at(&self) -> usize {
        self.tokens[self.next].0
    }

    fn take(&mut self) -> (usize, Token) {
        let t = self.tokens[self.next].clone();
        if t.1 != Token::End {
            self.next += 1;
        }
        t
    }

    fn eat(&mut self, symbol: &'static str) -> bool {
        let found = *self.peek() == Token::Symbol(symbol);
        if found {
            self.next += 1;
        }
        found
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Token::Name(n) if n.eq_ignore_ascii_case(word))
    }

    fn deeper(&mut self) -> Result<(), FormulaError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { fail(self.at(), "this formula is nested too deeply") } else { Ok(()) }
    }

    fn or(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.and()?;
        while self.keyword("or") {
            self.next += 1;
            left = Node::Binary { op: "OR", left: Box::new(left), right: Box::new(self.and()?) };
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.not()?;
        while self.keyword("and") {
            self.next += 1;
            left = Node::Binary { op: "AND", left: Box::new(left), right: Box::new(self.not()?) };
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Node, FormulaError> {
        if self.keyword("not") {
            self.next += 1;
            self.deeper()?;
            let inner = self.not()?;
            self.depth -= 1;
            return Ok(Node::Not(Box::new(inner)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Node, FormulaError> {
        let left = self.sum()?;
        if let Token::Symbol(op @ ("=" | "<>" | "<" | "<=" | ">" | ">=")) = *self.peek() {
            self.next += 1;
            return Ok(Node::Binary { op, left: Box::new(left), right: Box::new(self.sum()?) });
        }
        Ok(left)
    }

    fn sum(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.product()?;
        while let Token::Symbol(op @ ("+" | "-")) = *self.peek() {
            self.next += 1;
            left = Node::Binary { op, left: Box::new(left), right: Box::new(self.product()?) };
        }
        Ok(left)
    }

    fn product(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.unary()?;
        while let Token::Symbol(op @ ("*" | "/")) = *self.peek() {
            self.next += 1;
            left = Node::Binary { op, left: Box::new(left), right: Box::new(self.unary()?) };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Node, FormulaError> {
        if *self.peek() == Token::Symbol("-") {
            self.next += 1;
            self.deeper()?;
            let inner = self.unary()?;
            self.depth -= 1;
            return Ok(Node::Neg(Box::new(inner)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Node, FormulaError> {
        let (at, token) = self.take();
        match token {
            Token::Number(n) => Ok(Node::Number(n)),
            Token::Text(t) => Ok(Node::Text(t)),
            Token::Symbol("(") => {
                self.deeper()?;
                let inner = self.or()?;
                self.depth -= 1;
                if !self.eat(")") {
                    return fail(self.at(), "a \")\" is missing here");
                }
                Ok(inner)
            }
            Token::Name(name) => {
                let lower = name.to_ascii_lowercase();
                match lower.as_str() {
                    "true" => return Ok(Node::Bool(true)),
                    "false" => return Ok(Node::Bool(false)),
                    "null" => return Ok(Node::Null),
                    "and" | "or" | "not" => return fail(at, format!("{name:?} needs something before it")),
                    _ => {}
                }
                if !self.eat("(") {
                    return Ok(Node::Field { name, at });
                }
                if AGGREGATIONS.contains(&lower.as_str()) {
                    let column = match self.take() {
                        (_, Token::Name(c)) if self.eat(")") => c,
                        _ => {
                            return fail(at, format!("{lower}(…) takes one column, such as {lower}(amount)"));
                        }
                    };
                    return Ok(Node::Field { name: format!("{lower}({column})"), at });
                }
                if !FUNCTIONS.contains(&lower.as_str())
                    && !RESULT_FUNCTIONS.contains(&lower.as_str())
                    && !WINDOW_OPTIONS.contains(&lower.as_str())
                {
                    return fail(at, format!("unknown function {lower:?}; formulas can use {}", FUNCTIONS.join(", ")));
                }
                self.deeper()?;
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.or()?);
                        if self.eat(")") {
                            break;
                        }
                        if !self.eat(",") {
                            return fail(self.at(), "a \",\" or \")\" is missing here");
                        }
                    }
                }
                self.depth -= 1;
                Ok(Node::Call { name: lower, args, at })
            }
            Token::End => fail(at, "the formula ends too early: something is missing at the end"),
            Token::Symbol(s) => fail(at, format!("{s:?} isn't expected here")),
        }
    }
}

fn parse(expr: &str) -> Result<Node, FormulaError> {
    if expr.trim().is_empty() {
        return fail(0, "the formula is empty");
    }
    let mut p = Parser { tokens: tokens(expr)?, next: 0, depth: 0 };
    let node = p.or()?;
    if *p.peek() != Token::End {
        return fail(p.at(), "an operator (such as + or *) is missing here");
    }
    Ok(node)
}

/// The fields and aggregations a formula uses, each once, in order.
pub fn references(expr: &str) -> Result<Vec<String>, FormulaError> {
    let mut out = vec![];
    walk(&parse(expr)?, &mut |n| {
        if let Node::Field { name, .. } = n
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    });
    Ok(out)
}

/// The functions a formula calls.
pub fn calls(expr: &str) -> Result<Vec<String>, FormulaError> {
    let mut out = vec![];
    walk(&parse(expr)?, &mut |n| {
        if let Node::Call { name, .. } = n
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    });
    Ok(out)
}

/// Whether a formula calculates on a chart's results (running_sum, rank...).
pub fn is_on_results(expr: &str) -> Result<bool, FormulaError> {
    let mut found = false;
    walk(&parse(expr)?, &mut |n| {
        if let Node::Call { name, .. } = n {
            found |= RESULT_FUNCTIONS.contains(&name.as_str());
        }
    });
    Ok(found)
}

/// The formula written again, with each field's name changed by `name` (a blend's `model.field` to
/// the model's own `field`): fully bracketed, so it means the same.
pub fn rename(expr: &str, name: impl Fn(&str) -> String) -> Result<String, FormulaError> {
    fn write(n: &Node, name: &dyn Fn(&str) -> String) -> String {
        match n {
            Node::Number(x) => x.clone(),
            Node::Text(t) => format!("'{}'", t.replace('\'', "''")),
            Node::Bool(b) => b.to_string(),
            Node::Null => "null".into(),
            // an aggregation of a column: sum(amount)
            Node::Field { name: f, .. } => match f.split_once('(') {
                Some((agg, column)) => format!("{agg}({})", name(column.trim_end_matches(')'))),
                None => name(f),
            },
            Node::Neg(x) => format!("(-{})", write(x, name)),
            Node::Not(x) => format!("(not {})", write(x, name)),
            Node::Binary { op, left, right } => {
                format!("({} {} {})", write(left, name), op.to_lowercase(), write(right, name))
            }
            Node::Call { name: f, args, .. } => {
                format!("{f}({})", args.iter().map(|a| write(a, name)).collect::<Vec<_>>().join(", "))
            }
        }
    }
    Ok(write(&parse(expr)?, &name))
}

fn walk(n: &Node, visit: &mut impl FnMut(&Node)) {
    visit(n);
    match n {
        Node::Neg(x) | Node::Not(x) => walk(x, visit),
        Node::Binary { left, right, .. } => {
            walk(left, visit);
            walk(right, visit);
        }
        // along(…) and within(…) name dimensions, not values
        Node::Call { name, args, .. } if !WINDOW_OPTIONS.contains(&name.as_str()) => {
            args.iter().for_each(|a| walk(a, visit))
        }
        _ => {}
    }
}

/// The rows a calculation on results works over: `along` (the first date dimension, or the first
/// dimension) for running totals and the like, separately for each value of the `across`
/// dimensions; rank and % of total go across, within each value along. SQL column references.
pub struct Window {
    pub along: String,
    pub across: Vec<String>,
    /// the chart's dimensions (field name, column), which along(…) and within(…) may name
    pub dims: Vec<(String, String)>,
}

/// What a calculation on results may be told, after its value: along(a dimension), within(…).
const WINDOW_OPTIONS: [&str; 2] = ["along", "within"];

/// A calculation on a chart's results, as ScramDB SQL over the result's work table.
pub fn compile_on_results(
    expr: &str,
    window: &Window,
    resolve: &mut impl FnMut(&str) -> Result<(String, Type), String>,
) -> Result<(String, Type), FormulaError> {
    let node = parse(expr)?;
    let mut c =
        Compiler { dl: Dialect::Postgres, resolve, window: Some(window), inside: false, rows: None, in_row: false };
    let (sql, kind) = c.node(&node)?;
    match kind {
        Kind::Number | Kind::Any => Ok((sql, Type::Number)),
        _ => fail(0, "a calculation on results gives a number"),
    }
}

/// What a formula (or a field) gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Number,
    Text,
    Condition,
    Date,
    /// an instant (a `time` field)
    Time,
    /// a date and time of day, without a time zone (a `datetime` field)
    DateTime,
}

/// An aggregation of each row's value (sum_if and the like), which the caller writes as it writes
/// a measure's: so a period, and everything else a measure is computed with, applies the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Aggregate {
    Sum,
    Avg,
    Count,
    Min,
    Max,
    CountDistinct,
    /// of a sample, as the engines' STDDEV_SAMP and VAR_SAMP
    StdDev,
    Variance,
}

type Resolve<'a> = &'a mut dyn FnMut(&str) -> Result<(String, Type), String>;
type Aggregator<'a> = &'a mut dyn FnMut(Aggregate, &str) -> Result<String, String>;

/// A formula on totals that may also add up rows with a condition (sum_if and the like): `row`
/// resolves the columns of each row inside those, `aggregate` writes their aggregation.
pub fn compile_totals(
    expr: &str,
    dl: Dialect,
    resolve: &mut impl FnMut(&str) -> Result<(String, Type), String>,
    row: &mut impl FnMut(&str) -> Result<(String, Type), String>,
    aggregate: &mut impl FnMut(Aggregate, &str) -> Result<String, String>,
) -> Result<(String, Type), FormulaError> {
    let node = parse(expr)?;
    let mut c = Compiler { dl, resolve, window: None, inside: false, rows: Some((row, aggregate)), in_row: false };
    let (sql, kind) = c.node(&node)?;
    Ok((sql, kind_type(kind)))
}

fn kind_type(kind: Kind) -> Type {
    match kind {
        Kind::Text => Type::Text,
        Kind::Condition => Type::Condition,
        Kind::Date => Type::Date,
        Kind::Time => Type::Time,
        Kind::DateTime => Type::DateTime,
        Kind::Number | Kind::Any => Type::Number,
    }
}

/// The SQL of a formula, and what it gives. `resolve` gives the SQL of a field or aggregation and
/// its type, or why there's no such thing (it decides which fields a formula may use: measures
/// for a formula on totals, columns for one per row).
pub fn compile(
    expr: &str,
    dl: Dialect,
    resolve: &mut impl FnMut(&str) -> Result<(String, Type), String>,
) -> Result<(String, Type), FormulaError> {
    let node = parse(expr)?;
    let mut c = Compiler { dl, resolve, window: None, inside: false, rows: None, in_row: false };
    let (sql, kind) = c.node(&node)?;
    Ok((sql, kind_type(kind)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Text,
    Condition,
    Date,
    Time,
    DateTime,
    /// null: fits any
    Any,
}

impl Kind {
    fn is_date(self) -> bool {
        matches!(self, Kind::Date | Kind::Time | Kind::DateTime)
    }
}

impl From<Type> for Kind {
    fn from(t: Type) -> Kind {
        match t {
            Type::Number => Kind::Number,
            Type::Text => Kind::Text,
            Type::Condition => Kind::Condition,
            Type::Date => Kind::Date,
            Type::Time => Kind::Time,
            Type::DateTime => Kind::DateTime,
        }
    }
}

/// The kind both have (null fits any), if they agree.
fn same(a: Kind, b: Kind) -> Option<Kind> {
    match (a, b) {
        (Kind::Any, k) | (k, Kind::Any) => Some(k),
        (a, b) if a == b => Some(a),
        _ => None,
    }
}

struct Compiler<'a, F> {
    dl: Dialect,
    resolve: &'a mut F,
    /// calculating on results: the rows to work over
    window: Option<&'a Window>,
    /// inside a calculation on results (they don't nest)
    inside: bool,
    /// on totals: the row's columns, and how to aggregate them (sum_if and the like)
    rows: Option<(Resolve<'a>, Aggregator<'a>)>,
    /// inside sum_if and the like: names are the row's columns
    in_row: bool,
}

impl<F: FnMut(&str) -> Result<(String, Type), String>> Compiler<'_, F> {
    fn number(&mut self, n: &Node, at: usize) -> Result<String, FormulaError> {
        match self.node(n)? {
            (_, Kind::Condition) => fail(at, "a number is expected here, not a condition"),
            (_, Kind::Text) => fail(at, "a number is expected here, not text"),
            (_, k) if k.is_date() => fail(
                at,
                "a number is expected here, not a date: date_add(…) moves a date, days_between(…) counts the days",
            ),
            (sql, _) => Ok(sql),
        }
    }

    fn text(&mut self, n: &Node, at: usize) -> Result<String, FormulaError> {
        match self.node(n)? {
            (sql, Kind::Text | Kind::Any) => Ok(sql),
            _ => fail(at, "text is expected here"),
        }
    }

    /// A date, time or date-time, and which.
    fn date(&mut self, n: &Node, at: usize) -> Result<(String, Kind), FormulaError> {
        if let Node::Text(t) = n {
            let kind = if t.trim().len() > 10 { Kind::DateTime } else { Kind::Date };
            return Ok((self.written_date(t, kind, at)?, kind));
        }
        match self.node(n)? {
            (sql, k) if k.is_date() => Ok((sql, k)),
            _ => fail(at, "a date is expected here, such as a date field"),
        }
    }

    /// A date written in a formula ('2026-01-31', '2026-01-31 14:30'), as a date of `kind`.
    fn written_date(&self, t: &str, kind: Kind, at: usize) -> Result<String, FormulaError> {
        use chrono::{NaiveDate, NaiveDateTime};
        let t = t.trim();
        let when = NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M"))
            .or_else(|_| NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S"))
            .ok()
            .map(|d| (d, true))
            .or_else(|| {
                NaiveDate::parse_from_str(t, "%Y-%m-%d")
                    .ok()
                    .map(|d| (d.and_hms_opt(0, 0, 0).expect("midnight"), false))
            });
        let Some((when, timed)) = when else {
            return fail(at, format!("'{t}' isn't a date: write it as 2026-01-31, or 2026-01-31 14:30"));
        };
        let day = when.format("%Y-%m-%d");
        let instant = when.format("%Y-%m-%d %H:%M:%S");
        Ok(match (kind, self.dl) {
            (Kind::Date, _) if timed => return fail(at, format!("'{t}': a date has no time of day")),
            (Kind::Date, _) => format!("CAST('{day}' AS DATE)"),
            // instants are UTC
            (Kind::Time, Dialect::Postgres) => format!("CAST('{instant}+00' AS TIMESTAMPTZ)"),
            (Kind::Time, Dialect::BigQuery) => format!("CAST('{instant}+00' AS TIMESTAMP)"),
            (_, Dialect::BigQuery) => format!("CAST('{instant}' AS DATETIME)"),
            _ => format!("CAST('{instant}' AS TIMESTAMP)"),
        })
    }

    /// A function's unit, one of `units`, in quotes ('month').
    fn unit(&self, n: &Node, at: usize, name: &str, units: &[&'static str]) -> Result<&'static str, FormulaError> {
        let listed = match units.split_last() {
            Some((last, rest)) => {
                format!("{} or '{last}'", rest.iter().map(|u| format!("'{u}'")).collect::<Vec<_>>().join(", "))
            }
            None => String::new(),
        };
        match n {
            Node::Text(t) => match units.iter().find(|u| u.eq_ignore_ascii_case(t.trim())) {
                Some(u) => Ok(u),
                None => fail(at, format!("{name}(…)'s unit is {listed}")),
            },
            _ => fail(at, format!("{name}(…)'s unit goes in quotes, such as 'month': {listed}")),
        }
    }

    fn condition(&mut self, n: &Node, at: usize, what: &str) -> Result<String, FormulaError> {
        match self.node(n)? {
            (sql, Kind::Condition | Kind::Any) => Ok(sql),
            _ => fail(at, format!("{what} must be a condition, such as units > 0")),
        }
    }

    /// Values of one kind (numbers, or text), for if and coalesce.
    fn alike(&mut self, args: &[Node], at: usize, what: &str) -> Result<(Vec<String>, Kind), FormulaError> {
        let mut kind = Kind::Any;
        let mut out = vec![];
        for a in args {
            let (sql, k) = self.node(a)?;
            kind = match same(kind, k) {
                Some(Kind::Condition) => return fail(at, format!("{what} takes numbers or text, not conditions")),
                Some(k) => k,
                None => return fail(at, format!("{what} needs values of the same kind: all numbers, or all text")),
            };
            out.push(sql);
        }
        Ok((out, kind))
    }

    fn node(&mut self, n: &Node) -> Result<(String, Kind), FormulaError> {
        Ok(match n {
            Node::Number(x) => (x.clone(), Kind::Number),
            Node::Text(t) => match self.dl.quote(t) {
                Ok(q) => (q, Kind::Text),
                Err(e) => return fail(0, e.0),
            },
            Node::Bool(b) => ((if *b { "TRUE" } else { "FALSE" }).to_string(), Kind::Condition),
            Node::Null => ("NULL".to_string(), Kind::Any),
            Node::Field { name, at } => match match (self.in_row, self.rows.as_mut()) {
                (true, Some((row, _))) => row(name),
                _ => (self.resolve)(name),
            } {
                Ok((sql, t)) => (sql, t.into()),
                Err(why) => return fail(*at, why),
            },
            Node::Neg(x) => (format!("(-{})", self.number(x, 0)?), Kind::Number),
            Node::Not(x) => (format!("(NOT {})", self.condition(x, 0, "what follows not")?), Kind::Condition),
            Node::Binary { op: op @ ("AND" | "OR"), left, right } => {
                let what = format!("each side of {}", op.to_lowercase());
                let (l, r) = (self.condition(left, 0, &what)?, self.condition(right, 0, &what)?);
                (format!("({l} {op} {r})"), Kind::Condition)
            }
            Node::Binary { op: op @ ("+" | "-" | "*" | "/"), left, right } => {
                let (l, r) = (self.number(left, 0)?, self.number(right, 0)?);
                match *op {
                    "/" => (format!("(CAST({l} AS {}) / NULLIF({r}, 0))", self.dl.double()), Kind::Number),
                    _ => (format!("({l} {op} {r})"), Kind::Number),
                }
            }
            Node::Binary { op, left, right } => {
                let ((mut l, a), (mut r, b)) = (self.node(left)?, self.node(right)?);
                // a date compared with a written one ('2026-01-01'): that date, of its kind
                if let (true, Node::Text(t)) = (a.is_date(), right.as_ref()) {
                    r = self.written_date(t, a, 0)?;
                } else if let (Node::Text(t), true) = (left.as_ref(), b.is_date()) {
                    l = self.written_date(t, b, 0)?;
                }
                let b = if a.is_date() && matches!(right.as_ref(), Node::Text(_)) { a } else { b };
                let a = if b.is_date() && matches!(left.as_ref(), Node::Text(_)) { b } else { a };
                match same(a, b) {
                    Some(k) if k != Kind::Condition => (format!("({l} {op} {r})"), Kind::Condition),
                    _ => {
                        return fail(
                            0,
                            "compare numbers with numbers, text with text and dates with dates of the same kind \
                             (days_between(a, b) counts the days from one to the other)",
                        );
                    }
                }
            }
            Node::Call { name, args, at } => self.call(name, args, *at)?,
        })
    }

    /// A window function over the results.
    fn on_results(&mut self, name: &str, args: &[Node], at: usize) -> Result<(String, Kind), FormulaError> {
        let Some(w) = self.window else {
            return fail(at, format!("{name}(…) works on a chart's results: add it to a chart in Explore"));
        };
        if self.inside {
            return fail(at, format!("{name}(…) can't be used inside another calculation on results"));
        }
        // the value(s), then along(…) and within(…)
        let first_option = args.iter().position(is_option).unwrap_or(args.len());
        let (args, options) = args.split_at(first_option);
        if !options.iter().all(is_option) {
            return fail(at, format!("along(…) and within(…) come after what {name}(…) calculates"));
        }
        let (mut along_given, mut within_given) = (None, None);
        for o in options {
            let Node::Call { name: option, args: dims, .. } = o else { unreachable!("an option") };
            let cols = self.dimensions_named(option, dims, w, at)?;
            let slot = if option == "along" { &mut along_given } else { &mut within_given };
            if slot.replace(cols).is_some() {
                return fail(at, format!("{option}(…) is given once"));
            }
        }
        let along_given = match along_given {
            Some(cols) if cols.len() == 1 => cols.into_iter().next(),
            Some(_) => return fail(at, "along(…) takes one dimension of the chart, such as along(sold_at)"),
            None => None,
        };
        let n = match (name, args) {
            ("moving_avg", [_, Node::Number(n)]) => match n.parse::<u32>() {
                Ok(n) if (1..=1000).contains(&n) => n,
                _ => return fail(at, "moving_avg(…)'s number of rows must be a whole number from 1 to 1000"),
            },
            ("moving_avg", _) => {
                return fail(at, "moving_avg(…) takes a number and how many rows: moving_avg(revenue, 3)");
            }
            ("ntile", [_, Node::Number(n)]) => match n.parse::<u32>() {
                Ok(n) if (1..=1000).contains(&n) => n,
                _ => return fail(at, "ntile(…)'s number of groups must be a whole number from 1 to 1000"),
            },
            ("ntile", _) => return fail(at, "ntile(…) takes a number and how many groups: ntile(revenue, 4)"),
            (_, [_]) => 0,
            _ => return fail(at, format!("{name}(…) takes one number")),
        };
        self.inside = true;
        let x = self.number(&args[0], at)?;
        self.inside = false;
        let ranks = matches!(name, "rank" | "dense_rank" | "percent_of_total" | "ntile");
        if ranks && along_given.is_some() {
            return fail(
                at,
                format!("{name}(…) doesn't go along anything: within(…) says among which rows it compares"),
            );
        }
        // along a dimension (the first date's), separately within each value of the others
        let order = along_given.clone().unwrap_or_else(|| w.along.clone());
        let partition = match (within_given.clone(), &along_given) {
            (Some(cols), _) => cols,
            (None, Some(a)) => w.dims.iter().map(|(_, c)| c.clone()).filter(|c| c != a).collect(),
            (None, None) => w.across.clone(),
        };
        let partition_by =
            |cols: &[String]| if cols.is_empty() { String::new() } else { format!("PARTITION BY {}", cols.join(", ")) };
        let mut along = vec![];
        if !partition.is_empty() {
            along.push(partition_by(&partition));
        }
        if !order.is_empty() {
            along.push(format!("ORDER BY {order}"));
        }
        // ranks and shares: within each value along, unless told within what
        let across = match within_given {
            Some(cols) => partition_by(&cols),
            None if w.across.is_empty() || w.along.is_empty() => String::new(),
            None => format!("PARTITION BY {}", w.along),
        };
        let frame =
            |from: &str| [along.join(" "), format!("ROWS BETWEEN {from} AND CURRENT ROW")].join(" ").trim().to_string();
        let sql = match name {
            "running_sum" => format!("SUM({x}) OVER ({})", frame("UNBOUNDED PRECEDING")),
            "running_avg" => format!("AVG({x}) OVER ({})", frame("UNBOUNDED PRECEDING")),
            "next" => format!("LEAD({x}) OVER ({})", along.join(" ")),
            "difference" => format!("({x} - LAG({x}) OVER ({}))", along.join(" ")),
            "percent_change" => {
                let before = format!("LAG({x}) OVER ({})", along.join(" "));
                format!("(CAST(({x} - {before}) AS {}) / NULLIF({before}, 0))", self.dl.double())
            }
            "dense_rank" => {
                format!("DENSE_RANK() OVER ({})", [across.clone(), format!("ORDER BY {x} DESC")].join(" ").trim())
            }
            "ntile" => format!("NTILE({n}) OVER ({})", [across.clone(), format!("ORDER BY {x} DESC")].join(" ").trim()),
            "first" | "last" => {
                let whole = [along.join(" "), "ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING".into()];
                format!("{}_VALUE({x}) OVER ({})", name.to_uppercase(), whole.join(" ").trim())
            }
            "moving_avg" => format!("AVG({x}) OVER ({})", frame(&format!("{} PRECEDING", n - 1))),
            "previous" => format!("LAG({x}) OVER ({})", along.join(" ")),
            "rank" => format!("RANK() OVER ({})", [across, format!("ORDER BY {x} DESC")].join(" ").trim()),
            _ => format!("(CAST({x} AS {}) / NULLIF(SUM({x}) OVER ({across}), 0))", self.dl.double()),
        };
        Ok((sql, Kind::Number))
    }

    /// The columns of the chart's dimensions an option names.
    fn dimensions_named(
        &self,
        option: &str,
        dims: &[Node],
        w: &Window,
        at: usize,
    ) -> Result<Vec<String>, FormulaError> {
        let names = || w.dims.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
        dims.iter()
            .map(|d| match d {
                Node::Field { name, .. } => match w.dims.iter().find(|(n, _)| n == name) {
                    Some((_, col)) => Ok(col.clone()),
                    None => fail(at, format!("{option}(…) takes the chart's dimensions: {}", names())),
                },
                _ if option == "along" => fail(at, "along(…) takes one dimension of the chart, such as along(sold_at)"),
                _ => fail(at, format!("{option}(…) takes the chart's dimensions: {}", names())),
            })
            .collect()
    }

    /// An aggregation of the rows' values (sum_if and the like): their parts are the row's columns.
    fn over_rows(&mut self, name: &str, args: &[Node], at: usize) -> Result<(String, Kind), FormulaError> {
        if self.in_row {
            return fail(at, format!("{name}(…) can't be used inside another that adds up rows"));
        }
        if self.rows.is_none() {
            return fail(at, format!("{name}(…) adds up rows: it works in a formula on totals"));
        }
        self.in_row = true;
        let each = self.each_row(name, args, at);
        self.in_row = false;
        let (agg, sql) = each?;
        let (_, aggregate) = self.rows.as_mut().expect("checked above");
        match aggregate(agg, &sql) {
            Ok(sql) => Ok((sql, Kind::Number)),
            Err(why) => fail(at, why),
        }
    }

    /// What sum_if and the like aggregate: each row's value, or nothing where the condition fails.
    fn each_row(&mut self, name: &str, args: &[Node], at: usize) -> Result<(Aggregate, String), FormulaError> {
        let count = |ok: bool, how: &str| if ok { Ok(()) } else { fail(at, format!("{name}(…) takes {how}")) };
        match name {
            "sum_if" | "avg_if" | "min_if" | "max_if" => {
                count(args.len() == 2, &format!("a number and a condition: {name}(amount, region = 'EMEA')"))?;
                let x = self.number(&args[0], at)?;
                let when = self.condition(&args[1], at, &format!("{name}(…)'s second part"))?;
                let agg = match name {
                    "sum_if" => Aggregate::Sum,
                    "avg_if" => Aggregate::Avg,
                    "min_if" => Aggregate::Min,
                    _ => Aggregate::Max,
                };
                Ok((agg, format!("CASE WHEN {when} THEN {x} END")))
            }
            "count_distinct_if" => {
                count(args.len() == 2, "a value and a condition: count_distinct_if(store, region = 'EMEA')")?;
                let x = match self.node(&args[0])? {
                    (_, Kind::Condition) => return fail(at, "count_distinct_if(…) counts values, not conditions"),
                    (sql, _) => sql,
                };
                let when = self.condition(&args[1], at, "count_distinct_if(…)'s second part")?;
                Ok((Aggregate::CountDistinct, format!("CASE WHEN {when} THEN {x} END")))
            }
            "stddev" | "variance" => {
                count(args.len() == 1, &format!("one number: {name}(amount)"))?;
                let agg = if name == "stddev" { Aggregate::StdDev } else { Aggregate::Variance };
                Ok((agg, self.number(&args[0], at)?))
            }
            _ => {
                count(args.len() == 1, "a condition: count_if(region = 'EMEA')")?;
                let when = self.condition(&args[0], at, "count_if(…)'s condition")?;
                Ok((Aggregate::Count, format!("CASE WHEN {when} THEN 1 END")))
            }
        }
    }

    fn call(&mut self, name: &str, args: &[Node], at: usize) -> Result<(String, Kind), FormulaError> {
        if RESULT_FUNCTIONS.contains(&name) {
            return self.on_results(name, args, at);
        }
        if WINDOW_OPTIONS.contains(&name) {
            return fail(
                at,
                format!("{name}(…) goes inside a calculation on results, such as running_sum(revenue, along(sold_at))"),
            );
        }
        let count = |ok: bool, how: &str| if ok { Ok(()) } else { fail(at, format!("{name}(…) takes {how}")) };
        match name {
            "if" => {
                count(args.len() == 3, "3 parts: if(condition, then, else)")?;
                let when = self.condition(&args[0], at, "the first part of if(…)")?;
                let (v, kind) = self.alike(&args[1..], at, "if(…)")?;
                Ok((format!("CASE WHEN {when} THEN {} ELSE {} END", v[0], v[1]), kind))
            }
            "coalesce" => {
                count(args.len() >= 2, "two or more values")?;
                let (v, kind) = self.alike(args, at, "coalesce(…)")?;
                Ok((format!("COALESCE({})", v.join(", ")), kind))
            }
            "round" => {
                count(matches!(args.len(), 1 | 2), "a number and, if you like, the decimal places")?;
                let places = match args.get(1) {
                    None => 0,
                    Some(Node::Number(p)) if p.parse::<u8>().is_ok_and(|p| p <= 10) => p.parse::<u8>().unwrap(),
                    Some(_) => return fail(at, "round(…)'s decimal places must be a whole number from 0 to 10"),
                };
                let x = self.number(&args[0], at)?;
                Ok(match self.dl {
                    // PostgreSQL rounds to decimal places only as NUMERIC
                    Dialect::Postgres => (format!("ROUND(CAST({x} AS NUMERIC), {places})"), Kind::Number),
                    _ => (format!("ROUND({x}, {places})"), Kind::Number),
                })
            }
            "abs" => {
                count(args.len() == 1, "one number")?;
                Ok((format!("ABS({})", self.number(&args[0], at)?), Kind::Number))
            }
            "lower" | "upper" => {
                count(args.len() == 1, "one text")?;
                Ok((format!("{}({})", name.to_uppercase(), self.text(&args[0], at)?), Kind::Text))
            }
            "concat" => {
                count(args.len() >= 2, "two or more values")?;
                let mut parts = vec![];
                for a in args {
                    parts.push(match self.node(a)? {
                        (sql, Kind::Text | Kind::Any) => sql,
                        (_, Kind::Condition) => return fail(at, "concat(…) joins text and numbers, not conditions"),
                        (sql, _) => format!("CAST({sql} AS {})", self.dl.text()),
                    });
                }
                Ok((format!("CONCAT({})", parts.join(", ")), Kind::Text))
            }
            f if OVER_ROWS.contains(&f) => self.over_rows(name, args, at),
            "floor" | "ceil" | "sqrt" | "ln" | "log10" | "exp" => {
                count(args.len() == 1, "one number")?;
                let x = self.number(&args[0], at)?;
                // what can't be computed is empty, as dividing by zero, not an error
                Ok((
                    match name {
                        "sqrt" => format!("CASE WHEN {x} >= 0 THEN SQRT({x}) END"),
                        "ln" | "log10" => format!("CASE WHEN {x} > 0 THEN {}({x}) END", name.to_uppercase()),
                        _ => format!("{}({x})", name.to_uppercase()),
                    },
                    Kind::Number,
                ))
            }
            "mod" | "power" => {
                count(args.len() == 2, &format!("two numbers: {name}(qty, 2)"))?;
                let (a, b) = (self.number(&args[0], at)?, self.number(&args[1], at)?);
                Ok((
                    match (name, self.dl) {
                        ("mod", Dialect::DuckDb) => format!("({a} % NULLIF({b}, 0))"),
                        ("mod", _) => format!("MOD(CAST({a} AS NUMERIC), CAST(NULLIF({b}, 0) AS NUMERIC))"),
                        // a negative number to a fraction has no real answer
                        _ => format!("CASE WHEN {a} < 0 AND {b} <> FLOOR({b}) THEN NULL ELSE POWER({a}, {b}) END"),
                    },
                    Kind::Number,
                ))
            }
            "substring" => {
                count(args.len() == 3, "a text, where to start and how many characters: substring(store, 1, 3)")?;
                let t = self.text(&args[0], at)?;
                let (Some(from), Some(n)) = (whole(&args[1]), whole(&args[2])) else {
                    return fail(at, "substring(…)'s start and length are whole numbers: start from 1");
                };
                if !(1..=10_000).contains(&from) || !(0..=10_000).contains(&n) {
                    return fail(at, "substring(…) starts from 1 (the first character), up to 10000 characters");
                }
                Ok((format!("SUBSTR({t}, {from}, {n})"), Kind::Text))
            }
            "position" => {
                count(args.len() == 2, "a text and the text to find: position(store, 'Centro')")?;
                let (t, x) = (self.text(&args[0], at)?, self.text(&args[1], at)?);
                Ok((format!("STRPOS({t}, {x})"), Kind::Number))
            }
            "split_part" => {
                count(args.len() == 3, "a text, what separates its parts, and which: split_part(code, '-', 2)")?;
                let (t, sep) = (self.text(&args[0], at)?, self.text(&args[1], at)?);
                let Some(n) = whole(&args[2]).filter(|n| (1..=1000).contains(n)) else {
                    return fail(at, "split_part(…)'s part is a whole number from 1 (the first)");
                };
                Ok((
                    match self.dl {
                        Dialect::BigQuery => format!("COALESCE(SPLIT({t}, {sep})[SAFE_ORDINAL({n})], '')"),
                        _ => format!("SPLIT_PART({t}, {sep}, {n})"),
                    },
                    Kind::Text,
                ))
            }
            "to_number" => {
                count(args.len() == 1, "one text: to_number(code)")?;
                let t = format!("TRIM({})", self.text(&args[0], at)?);
                // text that isn't a number is empty, not an error
                Ok((
                    match self.dl {
                        Dialect::BigQuery => format!("SAFE_CAST({t} AS FLOAT64)"),
                        Dialect::DuckDb => format!("TRY_CAST({t} AS DOUBLE)"),
                        Dialect::Postgres => format!(
                            "CASE WHEN {t} ~ '^[-+]?([0-9]+[.]?[0-9]*|[.][0-9]+)([eE][-+]?[0-9]+)?$' \
                             THEN CAST({t} AS DOUBLE PRECISION) END"
                        ),
                    },
                    Kind::Number,
                ))
            }
            "to_text" => {
                count(args.len() == 1, "a number or a date: to_text(qty)")?;
                match self.node(&args[0])? {
                    (_, Kind::Condition) => fail(at, "to_text(…) takes a number or a date; if(…) words a condition"),
                    (sql, Kind::Text) => Ok((sql, Kind::Text)),
                    (sql, _) => Ok((format!("CAST({sql} AS {})", self.dl.text()), Kind::Text)),
                }
            }
            "format_date" => {
                count(args.len() == 2, "a date and a pattern: format_date(sold_on, 'YYYY-MM')")?;
                let (x, k) = self.date(&args[0], at)?;
                let Node::Text(pattern) = &args[1] else {
                    return fail(at, "format_date(…)'s pattern goes in quotes, such as 'YYYY-MM'");
                };
                let parts = date_pattern(pattern).map_err(|e| FormulaError { at, message: e })?;
                if k == Kind::Date && parts.iter().any(|p| matches!(*p, "HH" | "MI")) {
                    return fail(at, "format_date(…)'s HH and MI needs a time; this is a date, without one");
                }
                let pg = |p: &str| match p {
                    "HH" => "HH24".to_string(),
                    other => other.to_string(),
                };
                let strf = |p: &str| match p {
                    "YYYY" => "%Y".to_string(),
                    "MM" => "%m".into(),
                    "DD" => "%d".into(),
                    "HH" => "%H".into(),
                    "MI" => "%M".into(),
                    "Mon" => "%b".into(),
                    "Dy" => "%a".into(),
                    other => other.to_string(),
                };
                let quote = |p: String| self.dl.quote(&p).map_err(|e| FormulaError { at, message: e.0 });
                Ok((
                    match (self.dl, k) {
                        (Dialect::Postgres, _) => {
                            format!("TO_CHAR({x}, {})", quote(parts.iter().map(|p| pg(p)).collect())?)
                        }
                        (Dialect::DuckDb, _) => {
                            format!("STRFTIME({x}, {})", quote(parts.iter().map(|p| strf(p)).collect())?)
                        }
                        (Dialect::BigQuery, kind) => {
                            let f = quote(parts.iter().map(|p| strf(p)).collect())?;
                            match kind {
                                Kind::Date => format!("FORMAT_DATE({f}, {x})"),
                                Kind::Time => format!("FORMAT_TIMESTAMP({f}, {x}, 'UTC')"),
                                _ => format!("FORMAT_DATETIME({f}, {x})"),
                            }
                        }
                    },
                    Kind::Text,
                ))
            }
            "fiscal_year" | "fiscal_quarter" => {
                count(args.len() == 2, &format!("a date and the month its year starts: {name}(sold_on, 4)"))?;
                let (x, k) = self.date(&args[0], at)?;
                let Some(m) = whole(&args[1]).filter(|m| (1..=12).contains(m)) else {
                    return fail(at, format!("{name}(…)'s month the year starts is 1 to 12 (4: April)"));
                };
                Ok((
                    match name {
                        // named after the year it ends in: from April, 2026-04-01 is in 2027's
                        "fiscal_year" => match (13 - m) % 12 {
                            0 => date_part(self.dl, "year", &x),
                            shift => date_part(self.dl, "year", &date_add(self.dl, &x, k, shift, "month")),
                        },
                        _ => match m - 1 {
                            0 => date_part(self.dl, "quarter", &x),
                            back => date_part(self.dl, "quarter", &date_add(self.dl, &x, k, -back, "month")),
                        },
                    },
                    Kind::Number,
                ))
            }
            "hours_between" | "minutes_between" => {
                count(args.len() == 2, &format!("two dates or times: {name}(started_at, ended_at)"))?;
                let (from, to) = (self.date(&args[0], at)?, self.date(&args[1], at)?);
                let per = if name == "hours_between" { 3600 } else { 60 };
                Ok((format!("({} / {per})", seconds_between(self.dl, from, to)), Kind::Number))
            }
            "contains" | "starts_with" | "ends_with" => {
                count(args.len() == 2, &format!("a text and the text to look for: {name}(store, 'milano')"))?;
                let (t, x) = (self.text(&args[0], at)?, self.text(&args[1], at)?);
                // capitals aside, as the contains filter; no wildcards (% is a percent sign)
                let (lt, lx) = (format!("LOWER({t})"), format!("LOWER({x})"));
                let sql = match name {
                    "contains" => format!("(STRPOS({lt}, {lx}) > 0)"),
                    "starts_with" => format!("(SUBSTR({lt}, 1, LENGTH({x})) = {lx})"),
                    _ => format!("(RIGHT({lt}, LENGTH({x})) = {lx})"),
                };
                Ok((sql, Kind::Condition))
            }
            "length" => {
                count(args.len() == 1, "one text")?;
                Ok((format!("LENGTH({})", self.text(&args[0], at)?), Kind::Number))
            }
            "trim" => {
                count(args.len() == 1, "one text")?;
                Ok((format!("TRIM({})", self.text(&args[0], at)?), Kind::Text))
            }
            "left" | "right" => {
                count(args.len() == 2, &format!("a text and how many characters: {name}(store, 3)"))?;
                let t = self.text(&args[0], at)?;
                let Some(n) = whole(&args[1]).filter(|n| (0..=10_000).contains(n)) else {
                    return fail(at, format!("{name}(…)'s how many is a whole number from 0 to 10000"));
                };
                Ok((format!("{}({t}, {n})", name.to_uppercase()), Kind::Text))
            }
            "replace" => {
                count(args.len() == 3, "a text, what to replace and with what: replace(store, 'Centro', 'Nord')")?;
                let parts = args.iter().map(|a| self.text(a, at)).collect::<Result<Vec<_>, _>>()?;
                Ok((format!("REPLACE({})", parts.join(", ")), Kind::Text))
            }
            "in" => {
                count(args.len() >= 2, "a value and one or more to look for: in(region, 'EMEA', 'APAC')")?;
                // a date among written ones
                let (first, kind) = self.node(&args[0])?;
                if kind.is_date() {
                    let mut v = vec![first];
                    for a in &args[1..] {
                        v.push(match a {
                            Node::Text(t) => self.written_date(t, kind, at)?,
                            other => match self.node(other)? {
                                (sql, k) if same(k, kind).is_some() => sql,
                                _ => return fail(at, "in(…) needs values of the same kind: all dates here"),
                            },
                        });
                    }
                    return Ok((format!("({} IN ({}))", v[0], v[1..].join(", ")), Kind::Condition));
                }
                let (v, _) = self.alike(args, at, "in(…)")?;
                Ok((format!("({} IN ({}))", v[0], v[1..].join(", ")), Kind::Condition))
            }
            "case" => {
                count(
                    args.len() >= 4 && args.len().is_multiple_of(2),
                    "a value, pairs of what it may be and what to give, then what to give else: \
                     case(region, 'EMEA', 'Europe', 'APAC', 'Asia', 'Other')",
                )?;
                // case(value, when, then, when, then, …, else)
                let (value, pairs, other) = (&args[0], &args[1..args.len() - 1], &args[args.len() - 1]);
                let whens: Vec<Node> = std::iter::once(value).chain(pairs.iter().step_by(2)).cloned().collect();
                let thens: Vec<Node> = pairs.iter().skip(1).step_by(2).chain([other]).cloned().collect();
                let (w, _) = self.alike(&whens, at, "case(…)'s value and what it may be")?;
                let (t, kind) = self.alike(&thens, at, "case(…)'s answers")?;
                let branches: Vec<String> = w[1..].iter().zip(&t).map(|(w, t)| format!("WHEN {w} THEN {t}")).collect();
                Ok((format!("CASE {} {} ELSE {} END", w[0], branches.join(" "), t[t.len() - 1]), kind))
            }
            part if DATE_PARTS.contains(&part) => {
                count(args.len() == 1, &format!("one date, such as {part}(sold_at)"))?;
                let (x, k) = self.date(&args[0], at)?;
                if part == "hour" && k == Kind::Date {
                    return fail(at, "hour(…) needs a time; this is a date, without one");
                }
                Ok((date_part(self.dl, part, &x), Kind::Number))
            }
            "today" => {
                count(args.is_empty(), "nothing, as today()")?;
                // written in, in UTC: the same on every engine (DuckDB's CURRENT_DATE needs ICU), and a
                // cached result is never yesterday's today
                Ok((format!("CAST('{}' AS DATE)", chrono::Utc::now().date_naive()), Kind::Date))
            }
            "days_between" => {
                count(args.len() == 2, "two dates: days_between(from, to)")?;
                let (from, to) = (self.date(&args[0], at)?, self.date(&args[1], at)?);
                Ok((days_between(self.dl, from, to), Kind::Number))
            }
            "date_diff" => {
                count(args.len() == 3, "two dates and a unit: date_diff(from, to, 'month')")?;
                let (from, to) = (self.date(&args[0], at)?, self.date(&args[1], at)?);
                let sql = match self.unit(&args[2], at, name, &["day", "month", "year"])? {
                    "day" => days_between(self.dl, from, to),
                    // calendar months and years: from March to April is one, whatever the day
                    unit => {
                        let years = format!(
                            "({} - {})",
                            date_part(self.dl, "year", &to.0),
                            date_part(self.dl, "year", &from.0)
                        );
                        match unit {
                            "year" => years,
                            _ => format!(
                                "({years} * 12 + {} - {})",
                                date_part(self.dl, "month", &to.0),
                                date_part(self.dl, "month", &from.0)
                            ),
                        }
                    }
                };
                Ok((sql, Kind::Number))
            }
            "date_trunc" => {
                count(args.len() == 2, "a date and a unit: date_trunc(sold_at, 'month')")?;
                let (x, k) = self.date(&args[0], at)?;
                let unit = self.unit(&args[1], at, name, &["hour", "day", "week", "month", "quarter", "year"])?;
                if unit == "hour" && k == Kind::Date {
                    return fail(at, "date_trunc(…, 'hour') needs a time; this is a date, without one");
                }
                Ok((self.dl.trunc(unit, &x, trunc_type(self.dl, k)), k))
            }
            "date_add" => {
                count(args.len() == 3, "a date, how many and a unit: date_add(sold_on, 30, 'day')")?;
                let (x, k) = self.date(&args[0], at)?;
                let Some(n) = whole(&args[1]).filter(|n| n.abs() <= 100_000) else {
                    return fail(at, "date_add(…)'s how many is a whole number, such as 30 or -7");
                };
                let unit = self.unit(&args[2], at, name, &["hour", "day", "week", "month", "year"])?;
                if unit == "hour" && k == Kind::Date {
                    return fail(at, "date_add(…, 'hour') needs a time; this is a date, without one");
                }
                Ok((date_add(self.dl, &x, k, n, unit), k))
            }
            _ => {
                count(args.len() >= 2, "two or more numbers")?;
                let parts = args.iter().map(|a| self.number(a, at)).collect::<Result<Vec<_>, _>>()?;
                Ok((format!("{}({})", name.to_uppercase(), parts.join(", ")), Kind::Number))
            }
        }
    }
}

fn is_option(n: &Node) -> bool {
    matches!(n, Node::Call { name, .. } if WINDOW_OPTIONS.contains(&name.as_str()))
}

/// A whole number written in the formula (minus included), such as 30 or -7.
fn whole(n: &Node) -> Option<i64> {
    match n {
        Node::Number(n) => n.parse().ok(),
        Node::Neg(inner) => whole(inner).map(|n| -n),
        _ => None,
    }
}

/// A part of a date as a whole number: weeks as in ISO 8601, weekdays from 1 (Monday) to 7.
fn date_part(dl: Dialect, part: &str, x: &str) -> String {
    match (dl, part) {
        // BigQuery's day of the week starts on Sunday (1)
        (Dialect::BigQuery, "weekday") => format!("(MOD(EXTRACT(DAYOFWEEK FROM {x}) + 5, 7) + 1)"),
        (Dialect::BigQuery, "week") => format!("EXTRACT(ISOWEEK FROM {x})"),
        (Dialect::BigQuery, _) => format!("EXTRACT({} FROM {x})", part.to_uppercase()),
        _ => {
            let p = if part == "weekday" { "ISODOW".to_string() } else { part.to_uppercase() };
            format!("CAST(EXTRACT({p} FROM {x}) AS INTEGER)")
        }
    }
}

fn as_date((x, k): &(String, Kind)) -> String {
    if *k == Kind::Date { x.clone() } else { format!("CAST({x} AS DATE)") }
}

/// The days from one date to the other (negative when it's before).
fn days_between(dl: Dialect, from: (String, Kind), to: (String, Kind)) -> String {
    let (a, b) = (as_date(&from), as_date(&to));
    match dl {
        Dialect::BigQuery => format!("DATE_DIFF({b}, {a}, DAY)"),
        Dialect::DuckDb => format!("DATE_DIFF('day', {a}, {b})"),
        Dialect::Postgres => format!("({b} - {a})"),
    }
}

/// format_date's pattern, in parts: YYYY, MM, DD, HH, MI, Mon, Dy, and separators (the same on
/// every engine: letters mean different things to each, so no others).
fn date_pattern(t: &str) -> Result<Vec<&'static str>, String> {
    const PARTS: [&str; 7] = ["YYYY", "Mon", "MM", "MI", "DD", "HH", "Dy"];
    const SEPARATORS: [&str; 8] = [" ", "-", "/", ".", ":", ",", "(", ")"];
    let mut out = vec![];
    let mut rest = t;
    while !rest.is_empty() {
        let Some(p) = PARTS.iter().chain(&SEPARATORS).find(|p| rest.starts_with(**p)) else {
            return Err(format!(
                "format_date(…) understands YYYY, MM, DD, HH, MI, Mon (Mar), Dy (Sun) and - / . : , spaces; not {rest:?}"
            ));
        };
        out.push(*p);
        rest = &rest[p.len()..];
    }
    if out.len() > 40 {
        return Err("format_date(…)'s pattern is at most 40 parts".into());
    }
    Ok(out)
}

/// The seconds from one date or time to another, as a number.
fn seconds_between(dl: Dialect, from: (String, Kind), to: (String, Kind)) -> String {
    match dl {
        Dialect::BigQuery => {
            let ts = |(x, k): (String, Kind)| if k == Kind::Time { x } else { format!("TIMESTAMP({x})") };
            format!("TIMESTAMP_DIFF({}, {}, SECOND)", ts(to), ts(from))
        }
        Dialect::DuckDb => format!("(EPOCH(CAST({} AS TIMESTAMP)) - EPOCH(CAST({} AS TIMESTAMP)))", to.0, from.0),
        Dialect::Postgres => {
            format!("EXTRACT(EPOCH FROM (CAST({} AS TIMESTAMP) - CAST({} AS TIMESTAMP)))", to.0, from.0)
        }
    }
}

/// The dimension type `Dialect::trunc` takes for a kind of date (DuckDB reads instants as UTC
/// timestamps: see `compiler::col_expr`).
fn trunc_type(dl: Dialect, k: Kind) -> &'static str {
    match k {
        Kind::Date => "date",
        Kind::Time if dl != Dialect::DuckDb => "time",
        _ => "datetime",
    }
}

/// A date moved by `n` units, still a date of its kind.
fn date_add(dl: Dialect, x: &str, k: Kind, n: i64, unit: &str) -> String {
    let u = unit.to_uppercase();
    match (dl, k) {
        (Dialect::BigQuery, Kind::Date) => format!("DATE_ADD({x}, INTERVAL {n} {u})"),
        (Dialect::BigQuery, Kind::Time) => match unit {
            "hour" | "day" => format!("TIMESTAMP_ADD({x}, INTERVAL {n} {u})"),
            // a timestamp adds days at most: weeks as days, months and years on its date-time
            "week" => format!("TIMESTAMP_ADD({x}, INTERVAL {} DAY)", n * 7),
            _ => format!("TIMESTAMP(DATETIME_ADD(DATETIME({x}), INTERVAL {n} {u}))"),
        },
        (Dialect::BigQuery, _) => format!("DATETIME_ADD({x}, INTERVAL {n} {u})"),
        (_, Kind::Date) => format!("CAST(({x} + INTERVAL '{n} {unit}') AS DATE)"),
        _ => format!("({x} + INTERVAL '{n} {unit}')"),
    }
}

/// The name most like a mistyped one, for "did you mean …?"
pub fn closest<'a>(name: &str, names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    fn distance(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + usize::from(ca != *cb));
                prev = row[j + 1];
                row[j + 1] = next;
            }
        }
        row[b.len()]
    }
    names
        .into_iter()
        .map(|n| (distance(name, n), n))
        .filter(|(d, n)| *d <= 2.max(n.len() / 3))
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::compiler::Dialect;

    /// Measures as their SQL, and ad-hoc aggregations spelled out.
    fn resolve(name: &str) -> Result<(String, Type), String> {
        let sql = match name {
            "revenue" => "SUM(amount)",
            "cost" => "SUM(cost)",
            "transactions" => "COUNT(*)",
            "units" => "SUM(qty)",
            "sum(amount)" => "SUM(\"amount\")",
            "count_distinct(store)" => "COUNT(DISTINCT \"store\")",
            other => return Err(format!("unknown field {other:?}")),
        };
        Ok((sql.into(), Type::Number))
    }

    /// Aggregations of each row's values, spelled out.
    fn aggregate(a: Aggregate, x: &str) -> Result<String, String> {
        Ok(match a {
            Aggregate::Sum => format!("SUM({x})"),
            Aggregate::Avg => format!("AVG({x})"),
            Aggregate::Count => format!("COUNT({x})"),
            Aggregate::Min => format!("MIN({x})"),
            Aggregate::Max => format!("MAX({x})"),
            Aggregate::CountDistinct => format!("COUNT_DISTINCT({x})"),
            Aggregate::StdDev => format!("STDDEV({x})"),
            Aggregate::Variance => format!("VARIANCE({x})"),
        })
    }

    /// A formula on totals: a number.
    fn sql(expr: &str) -> Result<String, FormulaError> {
        match compile_totals(expr, Dialect::Postgres, &mut resolve, &mut row, &mut aggregate)? {
            (sql, Type::Number) => Ok(sql),
            (_, t) => Err(FormulaError { at: 0, message: format!("gives {t:?}") }),
        }
    }

    /// Columns of a row.
    fn row(name: &str) -> Result<(String, Type), String> {
        match name {
            "region" | "store" => Ok((format!("\"{name}\""), Type::Text)),
            "amount" | "qty" => Ok((format!("\"{name}\""), Type::Number)),
            "returned" => Ok(("\"returned\"".into(), Type::Condition)),
            "sold_at" => Ok(("\"sold_at\"".into(), Type::Time)),
            "sold_on" | "ordered_on" => Ok((format!("\"{name}\""), Type::Date)),
            "naive" => Ok(("\"naive\"".into(), Type::DateTime)),
            other => Err(format!("unknown column {other:?}")),
        }
    }

    fn per_row(expr: &str, dl: Dialect) -> Result<(String, Type), FormulaError> {
        compile(expr, dl, &mut row)
    }

    #[test]
    fn per_row_text_numbers_and_conditions() {
        let pg = |e| per_row(e, Dialect::Postgres);
        assert_eq!(pg("amount * qty").unwrap(), ("(\"amount\" * \"qty\")".into(), Type::Number));
        assert_eq!(
            pg("if(region = 'EMEA', amount, 0)").unwrap(),
            ("CASE WHEN (\"region\" = 'EMEA') THEN \"amount\" ELSE 0 END".into(), Type::Number)
        );
        assert_eq!(
            pg("if(returned, 'returned', store)").unwrap(),
            ("CASE WHEN \"returned\" THEN 'returned' ELSE \"store\" END".into(), Type::Text)
        );
        assert_eq!(pg("concat(store, ' · ', region)").unwrap().0, "CONCAT(\"store\", ' · ', \"region\")");
        assert_eq!(pg("concat(store, qty)").unwrap().0, "CONCAT(\"store\", CAST(\"qty\" AS TEXT))");
        assert_eq!(pg("upper(lower(region))").unwrap(), ("UPPER(LOWER(\"region\"))".into(), Type::Text));
        assert_eq!(pg("amount > 100 and not returned").unwrap().1, Type::Condition);
        assert_eq!(pg("coalesce(region, 'none')").unwrap().1, Type::Text);
        // text is quoted for its engine
        assert_eq!(pg("'it''s'").unwrap().0, "'it''s'");
        assert_eq!(per_row("'it''s'", Dialect::BigQuery).unwrap().0, "'it\\'s'");
        assert_eq!(
            per_row("concat(store, qty)", Dialect::BigQuery).unwrap().0,
            "CONCAT(\"store\", CAST(\"qty\" AS STRING))"
        );
        // mistakes
        assert!(pg("region = 1").unwrap_err().message.contains("compare"));
        assert!(pg("region + 1").unwrap_err().message.contains("number"));
        assert!(pg("upper(amount)").unwrap_err().message.contains("text"));
        assert!(pg("if(returned, 'x', 1)").unwrap_err().message.contains("same kind"));
        assert!(pg("'unfinished").unwrap_err().message.contains("'"));
        assert!(pg("'nul\u{0}'").is_err());
    }

    #[test]
    fn arithmetic_on_measures() {
        assert_eq!(sql("revenue - cost").unwrap(), "(SUM(amount) - SUM(cost))");
        assert_eq!(sql("1 + 2 * 3").unwrap(), "(1 + (2 * 3))");
        assert_eq!(sql("(1 + 2) * 3").unwrap(), "((1 + 2) * 3)");
        assert_eq!(sql("-revenue").unwrap(), "(-SUM(amount))");
        assert_eq!(sql("2.5e3 * units").unwrap(), "(2.5e3 * SUM(qty))");
        // division never fails on zero, and never truncates integers
        assert_eq!(
            sql("(revenue - cost) / revenue").unwrap(),
            "(CAST((SUM(amount) - SUM(cost)) AS DOUBLE PRECISION) / NULLIF(SUM(amount), 0))"
        );
        assert_eq!(
            sql("sum(amount) / count_distinct(store)").unwrap(),
            "(CAST(SUM(\"amount\") AS DOUBLE PRECISION) / NULLIF(COUNT(DISTINCT \"store\"), 0))"
        );
        assert!(sql("count()").unwrap_err().message.contains("one column"));
    }

    #[test]
    fn conditions_and_functions() {
        assert_eq!(sql("if(units > 0, revenue, 0)").unwrap(), "CASE WHEN (SUM(qty) > 0) THEN SUM(amount) ELSE 0 END");
        assert_eq!(
            sql("if(units >= 1 and not revenue = 0 or cost <> 1, 1, null)").unwrap(),
            "CASE WHEN (((SUM(qty) >= 1) AND (NOT (SUM(amount) = 0))) OR (SUM(cost) <> 1)) THEN 1 ELSE NULL END"
        );
        assert_eq!(sql("coalesce(revenue, 0)").unwrap(), "COALESCE(SUM(amount), 0)");
        assert_eq!(sql("abs(cost)").unwrap(), "ABS(SUM(cost))");
        assert_eq!(sql("least(revenue, cost, 10)").unwrap(), "LEAST(SUM(amount), SUM(cost), 10)");
        assert_eq!(sql("greatest(revenue, 0)").unwrap(), "GREATEST(SUM(amount), 0)");
        // round's places: PostgreSQL rounds NUMERIC; the others round doubles
        assert_eq!(sql("round(revenue, 2)").unwrap(), "ROUND(CAST(SUM(amount) AS NUMERIC), 2)");
        assert_eq!(compile("round(revenue)", Dialect::DuckDb, &mut resolve).unwrap().0, "ROUND(SUM(amount), 0)");
        // names are case-insensitive for functions, as in a spreadsheet
        assert_eq!(sql("IF(TRUE, 1, 2)").unwrap(), "CASE WHEN TRUE THEN 1 ELSE 2 END");
    }

    #[test]
    fn mistakes_are_explained_where_they_are() {
        let e = sql("1 + revnue").unwrap_err();
        assert_eq!(e.at, 4);
        assert!(e.message.contains("unknown field \"revnue\""), "{e:?}");
        assert_eq!(closest("revnue", ["cost", "revenue", "units"]), Some("revenue"));
        assert_eq!(closest("xyz", ["cost", "revenue"]), None);
        let e = sql("revenue - ").unwrap_err();
        assert!(e.message.contains("end"), "{e:?}");
        let e = sql("(revenue").unwrap_err();
        assert!(e.message.contains(")"), "{e:?}");
        let e = sql("revenue cost").unwrap_err();
        assert_eq!(e.at, 8);
        assert!(sql("median(revenue)").unwrap_err().message.contains("unknown function \"median\""));
        assert!(sql("if(units > 0, revenue)").unwrap_err().message.contains("3"));
        assert!(sql("round(revenue, 1.5)").unwrap_err().message.contains("whole number"));
        assert!(sql("revenue + (units > 1)").unwrap_err().message.contains("a number"));
        assert!(sql("if(revenue, 1, 2)").unwrap_err().message.contains("condition"));
        assert!(sql("").unwrap_err().message.contains("empty"));
    }

    #[test]
    fn nothing_but_the_language_reaches_sql() {
        for hostile in [
            "revenue; DROP TABLE users",
            "revenue -- comment",
            "revenue /* x */",
            "'text'",
            "'x' || revenue",
            "\"revenue\"",
            "revenue\u{0}",
            "1e400",
            "sum(amount; drop)",
            "sum(amount) over ()",
            "users.password_hash",
            "sum(\"amount\")",
            "`revenue`",
            "$$x$$",
        ] {
            assert!(sql(hostile).is_err(), "{hostile:?} compiled: {:?}", sql(hostile));
        }
        // deep nesting is refused before it can exhaust the stack
        let deep = format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000));
        assert!(sql(&deep).unwrap_err().message.contains("nested"));
        let long = "1+".repeat(2000) + "1";
        assert!(sql(&long).unwrap_err().message.contains("long"));
    }

    /// Columns of a query's result, as a work table on ScramDB.
    fn result(name: &str) -> Result<(String, Type), String> {
        match name {
            "revenue" => Ok(("\"c1\"".into(), Type::Number)),
            "units" => Ok(("\"c2\"".into(), Type::Number)),
            other => Err(format!("unknown field {other:?}")),
        }
    }

    fn on_results(expr: &str) -> Result<String, FormulaError> {
        compile_on_results(expr, &window(), &mut result).map(|(sql, _)| sql)
    }

    /// The chart: by sold_at (along, c0) and region (c3); store (c4) too when asked.
    fn window() -> Window {
        Window {
            along: "\"c0\"".into(),
            across: vec!["\"c3\"".into()],
            dims: vec![("sold_at".into(), "\"c0\"".into()), ("region".into(), "\"c3\"".into())],
        }
    }

    #[test]
    fn calculations_on_results_go_along_and_within_what_they_are_told() {
        let three = Window {
            across: vec!["\"c3\"".into(), "\"c4\"".into()],
            dims: [window().dims, vec![("store".into(), "\"c4\"".into())]].concat(),
            ..window()
        };
        let sql = |e: &str| compile_on_results(e, &three, &mut result).map(|(s, _)| s);
        let frame = "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW";
        // along the date, within the region only (not each store)
        assert_eq!(
            sql("running_sum(revenue, along(sold_at), within(region))").unwrap(),
            format!("SUM(\"c1\") OVER (PARTITION BY \"c3\" ORDER BY \"c0\" {frame})")
        );
        // along another dimension: within the rest
        assert_eq!(
            sql("running_sum(revenue, along(region))").unwrap(),
            format!("SUM(\"c1\") OVER (PARTITION BY \"c0\", \"c4\" ORDER BY \"c3\" {frame})")
        );
        // within nothing: the whole chart
        assert_eq!(
            sql("running_sum(revenue, within())").unwrap(),
            format!("SUM(\"c1\") OVER (ORDER BY \"c0\" {frame})")
        );
        assert_eq!(
            sql("moving_avg(revenue, 3, within(store))").unwrap(),
            "AVG(\"c1\") OVER (PARTITION BY \"c4\" ORDER BY \"c0\" ROWS BETWEEN 2 PRECEDING AND CURRENT ROW)"
        );
        assert_eq!(sql("previous(units, within())").unwrap(), "LAG(\"c2\") OVER (ORDER BY \"c0\")");
        // ranks and shares: within what's given
        assert_eq!(sql("rank(revenue, within())").unwrap(), "RANK() OVER (ORDER BY \"c1\" DESC)");
        assert_eq!(
            sql("dense_rank(revenue, within(region, store))").unwrap(),
            "DENSE_RANK() OVER (PARTITION BY \"c3\", \"c4\" ORDER BY \"c1\" DESC)"
        );
        assert_eq!(
            sql("percent_of_total(revenue, within(region))").unwrap(),
            "(CAST(\"c1\" AS DOUBLE PRECISION) / NULLIF(SUM(\"c1\") OVER (PARTITION BY \"c3\"), 0))"
        );
        // mistakes
        let e = |x: &str| sql(x).unwrap_err().message;
        assert!(
            e("rank(revenue, along(sold_at))").contains("doesn't go along"),
            "{}",
            e("rank(revenue, along(sold_at))")
        );
        assert!(e("running_sum(revenue, along(nope))").contains("sold_at, region, store"));
        assert!(e("running_sum(revenue, along(sold_at, region))").contains("one dimension"));
        assert!(e("running_sum(revenue, along(revenue + 1))").contains("one dimension"));
        assert!(
            e("running_sum(along(sold_at), revenue)").contains("after"),
            "{}",
            e("running_sum(along(sold_at), revenue)")
        );
        assert!(e("running_sum(revenue, within(region), within(store))").contains("once"));
        assert!(e("along(sold_at)").contains("inside a calculation on results"));
        assert!(
            per_row("within(region)", Dialect::Postgres)
                .unwrap_err()
                .message
                .contains("inside a calculation on results")
        );
        // the dimensions they name aren't measures the chart needs
        assert_eq!(references("running_sum(revenue, along(sold_at), within(region))").unwrap(), ["revenue"]);
    }

    #[test]
    fn calculations_on_results_are_window_functions() {
        let along = "PARTITION BY \"c3\" ORDER BY \"c0\"";
        assert_eq!(
            on_results("running_sum(revenue)").unwrap(),
            format!("SUM(\"c1\") OVER ({along} ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)")
        );
        assert_eq!(
            on_results("moving_avg(revenue, 3)").unwrap(),
            format!("AVG(\"c1\") OVER ({along} ROWS BETWEEN 2 PRECEDING AND CURRENT ROW)")
        );
        assert_eq!(on_results("previous(units)").unwrap(), format!("LAG(\"c2\") OVER ({along})"));
        // across: within each value of the first dimension
        assert_eq!(on_results("rank(revenue)").unwrap(), "RANK() OVER (PARTITION BY \"c0\" ORDER BY \"c1\" DESC)");
        assert_eq!(
            on_results("percent_of_total(revenue)").unwrap(),
            "(CAST(\"c1\" AS DOUBLE PRECISION) / NULLIF(SUM(\"c1\") OVER (PARTITION BY \"c0\"), 0))"
        );
        // mixed with arithmetic
        assert_eq!(
            on_results("revenue - previous(revenue)").unwrap(),
            format!("(\"c1\" - LAG(\"c1\") OVER ({along}))")
        );
        // the next value, the change and its percentage, the running average, ranks without gaps
        assert_eq!(on_results("next(units)").unwrap(), format!("LEAD(\"c2\") OVER ({along})"));
        assert_eq!(on_results("difference(revenue)").unwrap(), format!("(\"c1\" - LAG(\"c1\") OVER ({along}))"));
        assert_eq!(
            on_results("percent_change(revenue)").unwrap(),
            format!(
                "(CAST((\"c1\" - LAG(\"c1\") OVER ({along})) AS DOUBLE PRECISION) / NULLIF(LAG(\"c1\") OVER ({along}), 0))"
            )
        );
        assert_eq!(
            on_results("running_avg(revenue)").unwrap(),
            format!("AVG(\"c1\") OVER ({along} ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)")
        );
        assert_eq!(
            on_results("dense_rank(revenue)").unwrap(),
            "DENSE_RANK() OVER (PARTITION BY \"c0\" ORDER BY \"c1\" DESC)"
        );
        // the first and last values along, and quartiles across
        let whole = "ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING";
        assert_eq!(on_results("first(units)").unwrap(), format!("FIRST_VALUE(\"c2\") OVER ({along} {whole})"));
        assert_eq!(on_results("last(units)").unwrap(), format!("LAST_VALUE(\"c2\") OVER ({along} {whole})"));
        assert_eq!(
            on_results("ntile(revenue, 4)").unwrap(),
            "NTILE(4) OVER (PARTITION BY \"c0\" ORDER BY \"c1\" DESC)"
        );
        assert!(on_results("ntile(revenue)").unwrap_err().message.contains("ntile(…) takes"));
        assert!(on_results("ntile(revenue, 0)").unwrap_err().message.contains("1 to 1000"));
        // one dimension: nothing to partition by
        let w = Window { along: "\"c0\"".into(), across: vec![], dims: vec![] };
        assert_eq!(compile_on_results("rank(units)", &w, &mut result).unwrap().0, "RANK() OVER (ORDER BY \"c2\" DESC)");
        assert_eq!(
            compile_on_results("running_sum(units)", &w, &mut result).unwrap().0,
            "SUM(\"c2\") OVER (ORDER BY \"c0\" ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)"
        );
    }

    #[test]
    fn calculations_on_results_only_where_there_are_results() {
        // a formula on totals or per row has no results to work on
        assert!(sql("running_sum(revenue)").unwrap_err().message.contains("chart's results"));
        assert!(per_row("previous(amount)", Dialect::Postgres).unwrap_err().message.contains("chart's results"));
        assert!(on_results("running_sum(previous(revenue))").unwrap_err().message.contains("inside"));
        assert!(on_results("moving_avg(revenue, 0)").unwrap_err().message.contains("1 to 1000"));
        assert!(on_results("moving_avg(revenue)").unwrap_err().message.contains("moving_avg"));
        assert!(on_results("running_sum(revenue, 2)").unwrap_err().message.contains("one number"));
        // which formulas are calculations on results, and what they use
        assert!(is_on_results("revenue / running_sum(revenue)").unwrap());
        assert!(!is_on_results("revenue / units").unwrap());
        assert_eq!(references("previous(revenue) + sum(qty) - revenue").unwrap(), ["revenue", "sum(qty)"]);
    }

    /// The date functions' answers, on 2026-03-15 (a Sunday; 13:45 UTC) and 2025-11-30.
    const DATES: &[(&str, &str)] = &[
        ("year(sold_at)", "2026"),
        ("quarter(sold_at)", "1"),
        ("month(sold_on)", "3"),
        ("day(naive)", "15"),
        ("hour(sold_at)", "13"),
        ("hour(naive)", "13"),
        ("week(sold_on)", "11"),
        ("weekday(sold_on)", "7"),
        ("weekday(date_add(sold_on, 1, 'day'))", "1"),
        ("weekday(sold_at)", "7"),
        ("days_between(ordered_on, sold_on)", "105"),
        ("days_between(sold_on, ordered_on)", "-105"),
        ("days_between(ordered_on, sold_at)", "105"),
        ("date_diff(ordered_on, sold_on, 'month')", "4"),
        ("date_diff(ordered_on, sold_on, 'year')", "1"),
        ("date_diff(ordered_on, sold_at, 'day')", "105"),
        ("date_trunc(sold_on, 'month')", "2026-03-01"),
        ("date_trunc(sold_on, 'week')", "2026-03-09"),
        ("date_trunc(sold_at, 'quarter')", "2026-01-01"),
        ("date_trunc(naive, 'year')", "2026-01-01"),
        ("hour(date_trunc(naive, 'hour'))", "13"),
        ("date_add(sold_on, 1, 'month')", "2026-04-15"),
        ("date_add(sold_on, -2, 'week')", "2026-03-01"),
        ("date_add(sold_at, 1, 'month')", "2026-04-15"),
        ("hour(date_add(sold_at, 2, 'hour'))", "15"),
        ("date_add(naive, 1, 'year')", "2027-03-15"),
        ("date_add(sold_at, -1, 'week')", "2026-03-08"),
        ("if(sold_on > ordered_on, 'later', 'earlier')", "later"),
        ("days_between(today(), today())", "0"),
        ("if(year(today()) >= 2026, 'yes', 'no')", "yes"),
        ("coalesce(null, sold_on)", "2026-03-15"),
        // written dates, compared with dates of each kind
        ("if(sold_on >= '2026-01-01', 'yes', 'no')", "yes"),
        ("if(sold_at < '2026-03-15 13:00', 'yes', 'no')", "no"),
        ("if(sold_at > '2026-03-15', 'yes', 'no')", "yes"),
        ("if(naive = '2026-03-15 13:45:00', 'yes', 'no')", "yes"),
        ("days_between('2026-03-01', sold_on)", "14"),
        ("if(in(sold_on, '2026-03-14', '2026-03-15'), 'yes', 'no')", "yes"),
        // hours and minutes, as numbers (1.5: an hour and a half)
        ("hours_between(ordered_on, sold_on)", "2520"),
        ("hours_between(sold_on, sold_at)", "13.75"),
        ("minutes_between(sold_at, date_add(sold_at, 2, 'hour'))", "120"),
        ("minutes_between(naive, sold_at)", "0"),
        // dates as text, in a pattern of our own (the same on every engine)
        ("format_date(sold_at, 'YYYY-MM-DD HH:MI')", "2026-03-15 13:45"),
        ("format_date(sold_on, 'Mon YYYY')", "Mar 2026"),
        ("format_date(naive, 'Dy DD/MM')", "Sun 15/03"),
        // fiscal years, named after the year they end in
        ("fiscal_year(sold_on, 4)", "2026"),
        ("fiscal_year(date_add(sold_on, 1, 'month'), 4)", "2027"),
        ("fiscal_year(sold_at, 1)", "2026"),
        ("fiscal_quarter(sold_on, 4)", "4"),
        ("fiscal_quarter(sold_on, 1)", "1"),
        ("fiscal_quarter(ordered_on, 10)", "1"),
    ];

    /// The dates of [`DATES`] as each engine writes them.
    fn date_columns(dl: Dialect) -> impl FnMut(&str) -> Result<(String, Type), String> {
        move |name| {
            let time = match dl {
                // as the compiler reads a `time` column on DuckDB (col_expr): a UTC timestamp
                Dialect::DuckDb => "CAST(CAST('2026-03-15 13:45:00' AS TIMESTAMP) AS TIMESTAMP)".to_string(),
                Dialect::BigQuery => "TIMESTAMP('2026-03-15 13:45:00+00')".to_string(),
                Dialect::Postgres => "CAST('2026-03-15 13:45:00+00' AS TIMESTAMPTZ)".to_string(),
            };
            match name {
                "sold_at" => Ok((time, Type::Time)),
                "sold_on" => Ok(("CAST('2026-03-15' AS DATE)".into(), Type::Date)),
                "ordered_on" => Ok(("CAST('2025-11-30' AS DATE)".into(), Type::Date)),
                "naive" => Ok(("CAST('2026-03-15 13:45:00' AS TIMESTAMP)".into(), Type::DateTime)),
                other => Err(format!("unknown column {other:?}")),
            }
        }
    }

    /// Each of [`DATES`] as a query giving its answer as text (a date as its day).
    pub(crate) fn date_queries(dl: Dialect) -> Vec<(String, &'static str, &'static str)> {
        answer_queries(dl, DATES, &mut date_columns(dl))
    }

    /// The text functions' answers, on a sale in 'EMEA' from the store 'Milano Centro'.
    const TEXTS: &[(&str, &str)] = &[
        ("if(contains(store, 'milano'), 'yes', 'no')", "yes"),
        ("if(contains(store, 'MILANO'), 'yes', 'no')", "yes"),
        ("if(contains(store, 'roma'), 'yes', 'no')", "no"),
        // no wildcards: % is a percent sign
        ("if(contains(store, '%'), 'yes', 'no')", "no"),
        ("if(contains(store, lower(region)), 'yes', 'no')", "no"),
        ("if(starts_with(region, 'em'), 'yes', 'no')", "yes"),
        ("if(starts_with(region, 'EA'), 'yes', 'no')", "no"),
        ("if(ends_with(region, 'ea'), 'yes', 'no')", "yes"),
        ("if(ends_with(store, ''), 'yes', 'no')", "yes"),
        ("length(store)", "13"),
        ("length(cafe)", "4"),
        ("trim(padded)", "a b"),
        ("length(trim(padded))", "3"),
        ("left(store, 6)", "Milano"),
        ("right(store, 6)", "Centro"),
        ("concat('[', left(store, 0), ']')", "[]"),
        ("upper(left(cafe, 3))", "CAF"),
        ("replace(store, 'Centro', 'Nord')", "Milano Nord"),
        ("if(in(region, 'EMEA', 'APAC'), 'yes', 'no')", "yes"),
        ("if(in(region, 'AMER'), 'yes', 'no')", "no"),
        ("if(in(qty, 1, 2, 3), 'yes', 'no')", "yes"),
        ("case(region, 'EMEA', 'Europe', 'APAC', 'Asia', 'Other')", "Europe"),
        ("case(region, 'AMER', 'Americas', 'Other')", "Other"),
        ("case(qty, 1, 'one', 3, 'three', 'many')", "three"),
        ("case(region, 'AMER', 1, 0) + 1", "1"),
        ("concat(left(region, 2), '-', qty)", "EM-3"),
        ("substring(store, 8, 6)", "Centro"),
        ("concat('[', substring(store, 1, 0), ']')", "[]"),
        ("position(store, 'Centro')", "8"),
        ("position(store, 'Roma')", "0"),
        ("split_part(words, '-', 2)", "b"),
        ("concat('[', split_part(words, '-', 9), ']')", "[]"),
        ("to_number(digits) + 1", "43"),
        ("to_number(cost)", "12.5"),
        // not a number: empty, not an error
        ("to_number(bad)", "(empty)"),
        ("concat(to_text(qty), 'x')", "3x"),
        // numbers
        ("floor(half)", "2"),
        ("ceil(half)", "3"),
        ("floor(neg / 3)", "-2"),
        ("mod(qty, 2)", "1"),
        ("mod(neg, 3)", "-1"),
        ("mod(qty, 0)", "(empty)"),
        ("power(qty, 2)", "9"),
        ("power(neg, 2)", "16"),
        ("power(neg, 0.5)", "(empty)"),
        ("sqrt(qty * 3)", "3"),
        ("sqrt(neg)", "(empty)"),
        ("ln(exp(2))", "2"),
        ("log10(1000)", "3"),
        ("log10(0)", "(empty)"),
        ("exp(0)", "1"),
    ];

    /// The columns of [`TEXTS`], as literals for each engine.
    fn text_columns(dl: Dialect) -> impl FnMut(&str) -> Result<(String, Type), String> {
        move |name| {
            let text = |s: &str| Ok((dl.quote(s).unwrap(), Type::Text));
            match name {
                "region" => text("EMEA"),
                "store" => text("Milano Centro"),
                "padded" => text("  a b  "),
                "cafe" => text("Café"),
                "qty" => Ok(("3".into(), Type::Number)),
                "neg" => Ok(("(-4)".into(), Type::Number)),
                "half" => Ok(("2.5".into(), Type::Number)),
                "digits" => text("42"),
                "cost" => text(" 12.5 "),
                "bad" => text("12x"),
                "words" => text("a-b-c"),
                other => Err(format!("unknown column {other:?}")),
            }
        }
    }

    pub(crate) fn text_queries(dl: Dialect) -> Vec<(String, &'static str, &'static str)> {
        answer_queries(dl, TEXTS, &mut text_columns(dl))
    }

    /// An answer as expected: numbers as numbers (engines write 2, 2.0 or 2.000).
    pub(crate) fn same_answer(got: &str, want: &str) -> bool {
        match (got.trim().parse::<f64>(), want.parse::<f64>()) {
            (Ok(a), Ok(b)) => (a - b).abs() <= 1e-9 * b.abs().max(1.0),
            _ => got == want,
        }
    }

    /// Each formula as a query giving its answer as text (a date as its day; empty as "(empty)").
    fn answer_queries(
        dl: Dialect,
        cases: &[(&'static str, &'static str)],
        columns: &mut impl FnMut(&str) -> Result<(String, Type), String>,
    ) -> Vec<(String, &'static str, &'static str)> {
        cases
            .iter()
            .map(|(expr, want)| {
                let (sql, t) = compile(expr, dl, columns).unwrap_or_else(|e| panic!("{expr}: {e}"));
                let sql = match t {
                    Type::Date | Type::Time | Type::DateTime => format!("CAST({sql} AS DATE)"),
                    _ => sql,
                };
                (format!("SELECT COALESCE(CAST({sql} AS {}), '(empty)')", dl.text()), *expr, *want)
            })
            .collect()
    }

    /// On ScramDB (MAGNUM_TEST_DATABASE_URL) and a real PostgreSQL (MAGNUM_TEST_POSTGRES_URL);
    /// each skipped without its database.
    #[tokio::test]
    async fn date_and_text_functions_give_the_same_answers_on_scramdb_and_postgresql() {
        for var in ["MAGNUM_TEST_DATABASE_URL", "MAGNUM_TEST_POSTGRES_URL"] {
            let Ok(url) = std::env::var(var) else { continue };
            let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await.unwrap();
            tokio::spawn(conn);
            for (sql, expr, want) in date_queries(Dialect::Postgres).into_iter().chain(text_queries(Dialect::Postgres))
            {
                let row = client.query_one(&sql, &[]).await.unwrap_or_else(|e| panic!("{var}: {expr}: {sql}: {e}"));
                let got = row.get::<_, String>(0);
                assert!(same_answer(&got, want), "{var}: {expr}: {got:?}, not {want:?}: {sql}");
            }
        }
    }

    #[test]
    fn date_functions_on_bigquery() {
        let bq = |e| per_row(e, Dialect::BigQuery).unwrap().0;
        assert_eq!(bq("year(sold_at)"), "EXTRACT(YEAR FROM \"sold_at\")");
        assert_eq!(bq("week(sold_on)"), "EXTRACT(ISOWEEK FROM \"sold_on\")");
        // BigQuery counts from Sunday (1); Monday is 1 here, as elsewhere
        assert_eq!(bq("weekday(sold_on)"), "(MOD(EXTRACT(DAYOFWEEK FROM \"sold_on\") + 5, 7) + 1)");
        // today is written in (UTC): a cached result is never yesterday's today
        let today = chrono::Utc::now().date_naive();
        assert_eq!(bq("today()"), format!("CAST('{today}' AS DATE)"));
        assert_eq!(per_row("today()", Dialect::DuckDb).unwrap().0, format!("CAST('{today}' AS DATE)"));
        assert_eq!(
            bq("days_between(ordered_on, sold_at)"),
            "DATE_DIFF(CAST(\"sold_at\" AS DATE), \"ordered_on\", DAY)"
        );
        assert_eq!(bq("date_trunc(sold_at, 'month')"), "TIMESTAMP_TRUNC(\"sold_at\", MONTH, 'UTC')");
        assert_eq!(bq("date_add(sold_on, 2, 'month')"), "DATE_ADD(\"sold_on\", INTERVAL 2 MONTH)");
        assert_eq!(bq("date_add(naive, 2, 'hour')"), "DATETIME_ADD(\"naive\", INTERVAL 2 HOUR)");
        // a timestamp adds days and hours only: weeks as days, months through its date-time
        assert_eq!(bq("date_add(sold_at, 1, 'week')"), "TIMESTAMP_ADD(\"sold_at\", INTERVAL 7 DAY)");
        assert_eq!(
            bq("date_add(sold_at, -1, 'month')"),
            "TIMESTAMP(DATETIME_ADD(DATETIME(\"sold_at\"), INTERVAL -1 MONTH))"
        );
        // and the answers run the same, as the other engines' (the SQL is the one checked above)
        assert_eq!(date_queries(Dialect::BigQuery).len(), DATES.len());
    }

    #[test]
    fn date_mistakes_are_explained() {
        let pg = |e| per_row(e, Dialect::Postgres).unwrap_err().message;
        assert!(pg("hour(sold_on)").contains("hour(…) needs a time"), "{}", pg("hour(sold_on)"));
        assert!(pg("year(amount)").contains("a date is expected here"));
        assert!(pg("year(sold_on, 2)").contains("one date"));
        assert!(pg("date_trunc(sold_on, 'decade')").contains("'month'"));
        assert!(pg("date_trunc(sold_on, region)").contains("in quotes"));
        assert!(pg("date_trunc(sold_on, 'hour')").contains("needs a time"));
        assert!(pg("date_add(sold_on, qty, 'day')").contains("whole number"));
        assert!(pg("date_add(sold_on, 1.5, 'day')").contains("whole number"));
        assert!(pg("date_diff(sold_on, sold_at, 'week')").contains("'day', 'month' or 'year'"));
        assert!(pg("today(1)").contains("today()"));
        assert!(pg("sold_on + 1").contains("date_add"), "{}", pg("sold_on + 1"));
        assert!(pg("sold_on > sold_at").contains("days_between"), "{}", pg("sold_on > sold_at"));
        assert!(pg("sold_on > 3").contains("compare"));
        // what they give
        assert_eq!(per_row("date_trunc(sold_at, 'day')", Dialect::Postgres).unwrap().1, Type::Time);
        assert_eq!(per_row("date_add(sold_on, 1, 'day')", Dialect::Postgres).unwrap().1, Type::Date);
        assert_eq!(per_row("if(returned, naive, null)", Dialect::Postgres).unwrap().1, Type::DateTime);
        assert_eq!(per_row("concat('Q', quarter(sold_on))", Dialect::Postgres).unwrap().1, Type::Text);
        // on totals, dates come only through per-row formulas (for now)
        assert!(sql("year(revenue)").unwrap_err().message.contains("a date is expected here"));
    }

    /// The formula editor (frontend/src/formula.ts) offers every function, and only those.
    #[test]
    fn the_editor_offers_the_functions_there_are() {
        let editor = include_str!("../../frontend/src/formula.ts");
        let offered: Vec<&str> = editor.split("{ name: '").skip(1).filter_map(|rest| rest.split('\'').next()).collect();
        let mut there: Vec<&str> = FUNCTIONS.iter().chain(&RESULT_FUNCTIONS).chain(&WINDOW_OPTIONS).copied().collect();
        let mut offered = offered;
        there.sort_unstable();
        offered.sort_unstable();
        assert_eq!(offered, there);
    }

    #[test]
    fn text_functions_on_bigquery() {
        let bq = |e| per_row(e, Dialect::BigQuery).unwrap().0;
        assert_eq!(bq("contains(store, 'x')"), "(STRPOS(LOWER(\"store\"), LOWER('x')) > 0)");
        assert_eq!(bq("starts_with(region, 'em')"), "(SUBSTR(LOWER(\"region\"), 1, LENGTH('em')) = LOWER('em'))");
        assert_eq!(bq("left(store, 2)"), "LEFT(\"store\", 2)");
        assert_eq!(bq("in(region, 'a', 'b')"), "(\"region\" IN ('a', 'b'))");
        assert_eq!(bq("case(region, 'a', 1, 0)"), "CASE \"region\" WHEN 'a' THEN 1 ELSE 0 END");
        assert_eq!(text_queries(Dialect::BigQuery).len(), TEXTS.len());
    }

    #[test]
    fn text_mistakes_are_explained() {
        let pg = |e| per_row(e, Dialect::Postgres).unwrap_err().message;
        assert!(pg("contains(qty, 'x')").contains("text is expected here"));
        assert!(pg("contains(store)").contains("contains(…) takes"));
        assert!(pg("left(store, -1)").contains("whole number from 0"), "{}", pg("left(store, -1)"));
        assert!(pg("left(store, qty)").contains("whole number from 0"));
        assert!(pg("in(region)").contains("one or more"));
        assert!(pg("in(region, 1)").contains("same kind"));
        assert!(pg("case(region, 'EMEA', 1)").contains("else"), "{}", pg("case(region, 'EMEA', 1)"));
        assert!(pg("case(region, 1, 'x', 'y')").contains("same kind"));
        assert!(pg("case(region, 'EMEA', 1, 'x')").contains("same kind"));
        // what they give
        assert_eq!(per_row("in(region, 'a')", Dialect::Postgres).unwrap().1, Type::Condition);
        assert_eq!(per_row("length(store)", Dialect::Postgres).unwrap().1, Type::Number);
        assert_eq!(per_row("case(qty, 1, sold_on, null)", Dialect::Postgres).unwrap().1, Type::Date);
        // on totals: choices on measures
        assert_eq!(
            sql("case(units, 0, null, revenue / units)").unwrap(),
            "CASE SUM(qty) WHEN 0 THEN NULL ELSE (CAST(SUM(amount) AS DOUBLE PRECISION) / NULLIF(SUM(qty), 0)) END"
        );
        assert!(compile("in(units, 1, 2)", Dialect::Postgres, &mut resolve).is_ok());
    }

    #[test]
    fn totals_with_a_condition() {
        assert_eq!(
            sql("sum_if(amount, region = 'EMEA')").unwrap(),
            "SUM(CASE WHEN (\"region\" = 'EMEA') THEN \"amount\" END)"
        );
        assert_eq!(sql("count_if(returned)").unwrap(), "COUNT(CASE WHEN \"returned\" THEN 1 END)");
        assert_eq!(
            sql("avg_if(amount * qty, qty > 1)").unwrap(),
            "AVG(CASE WHEN (\"qty\" > 1) THEN (\"amount\" * \"qty\") END)"
        );
        // with the measures around them
        assert_eq!(
            sql("sum_if(amount, region = 'EMEA') / revenue").unwrap(),
            "(CAST(SUM(CASE WHEN (\"region\" = 'EMEA') THEN \"amount\" END) AS DOUBLE PRECISION) / NULLIF(SUM(amount), 0))"
        );
        // dates in the condition: the row's columns, as per row
        assert!(sql("count_if(year(sold_at) = 2026)").unwrap().contains("EXTRACT(YEAR FROM \"sold_at\")"));
    }

    #[test]
    fn totals_with_a_condition_mistakes() {
        let e = |x| sql(x).unwrap_err().message;
        assert!(e("sum_if(region, returned)").contains("number"));
        assert!(e("sum_if(amount, qty)").contains("condition"));
        assert!(e("sum_if(amount)").contains("sum_if(…) takes"));
        // inside, the row's columns: not measures
        assert!(e("sum_if(revenue, returned)").contains("unknown column"), "{}", e("sum_if(revenue, returned)"));
        assert!(e("sum_if(sum_if(amount, returned), returned)").contains("inside"));
        // they add up rows: not per row, nor on a chart's results
        let row = per_row("sum_if(amount, returned)", Dialect::Postgres).unwrap_err().message;
        assert!(row.contains("on totals"), "{row}");
        assert!(on_results("running_sum(count_if(returned))").unwrap_err().message.contains("on totals"));
    }

    #[test]
    fn blend_fields_are_names_with_a_dot() {
        assert_eq!(references("taxi.trips / weather.hours").unwrap(), ["taxi.trips", "weather.hours"]);
        assert_eq!(references("running_sum(taxi.trips, along(taxi.pickup_at))").unwrap(), ["taxi.trips"]);
        // a number's dot is still a number's
        assert_eq!(sql("revenue * .5").unwrap(), "(SUM(amount) * .5)");
        // one dot, between two names
        assert!(references("taxi.").is_err());
        assert!(references("a.b.c").is_err());
    }

    #[test]
    fn formulas_are_written_back_with_names_changed() {
        let strip = |n: &str| n.strip_prefix("t.").unwrap_or(n).to_string();
        assert_eq!(rename("t.revenue / t.units", strip).unwrap(), "(revenue / units)");
        assert_eq!(
            rename("if(t.units > 0 and not t.x, 'it''s', null)", strip).unwrap(),
            "if(((units > 0) and (not x)), 'it''s', null)"
        );
        assert_eq!(rename("sum(t.amount) + -t.cost", strip).unwrap(), "(sum(amount) + (-cost))");
        assert_eq!(
            rename("running_sum(t.revenue, along(t.sold_at), within())", strip).unwrap(),
            "running_sum(revenue, along(sold_at), within())"
        );
        // written back, a formula means the same
        for e in [
            "revenue / units - cost * 2",
            "if(units >= 1 or revenue = 0, 1.5, null)",
            "-(revenue - cost) / 3",
            "if(coalesce(revenue, 0) <> 0, 1, 0)",
            "round(revenue / units, 2)",
        ] {
            let again = rename(e, |n| n.to_string()).unwrap();
            assert_eq!(sql(&again).unwrap(), sql(e).unwrap(), "{e} -> {again}");
        }
        let row = rename("concat(region, ' / ', lower(store))", |n| n.to_string()).unwrap();
        assert_eq!(
            per_row(&row, Dialect::Postgres).unwrap(),
            per_row("concat(region, ' / ', lower(store))", Dialect::Postgres).unwrap()
        );
    }

    #[test]
    fn written_dates_numbers_and_text_on_bigquery() {
        let bq = |e| per_row(e, Dialect::BigQuery).unwrap().0;
        assert_eq!(bq("sold_on >= '2026-01-01'"), "(\"sold_on\" >= CAST('2026-01-01' AS DATE))");
        assert_eq!(bq("sold_at < '2026-03-15 13:00'"), "(\"sold_at\" < CAST('2026-03-15 13:00:00+00' AS TIMESTAMP))");
        assert_eq!(bq("to_number(store)"), "SAFE_CAST(TRIM(\"store\") AS FLOAT64)");
        assert_eq!(bq("split_part(store, '-', 2)"), "COALESCE(SPLIT(\"store\", '-')[SAFE_ORDINAL(2)], '')");
        assert!(
            bq("hours_between(sold_on, sold_at)").starts_with("(TIMESTAMP_DIFF("),
            "{}",
            bq("hours_between(sold_on, sold_at)")
        );
        assert_eq!(per_row("to_number(store)", Dialect::DuckDb).unwrap().0, "TRY_CAST(TRIM(\"store\") AS DOUBLE)");
    }

    #[test]
    fn written_dates_numbers_and_text_mistakes() {
        let pg = |e| per_row(e, Dialect::Postgres).unwrap_err().message;
        assert!(pg("sold_on > '2026-13-01'").contains("isn't a date"), "{}", pg("sold_on > '2026-13-01'"));
        assert!(pg("sold_on > 'yesterday'").contains("isn't a date"));
        assert!(pg("substring(store, 0, 2)").contains("from 1"));
        assert!(pg("split_part(store, '-', 0)").contains("from 1"));
        assert!(pg("to_number(qty)").contains("text is expected"));
        assert!(pg("mod(qty)").contains("mod(…) takes"));
        assert!(pg("sqrt(store)").contains("number"));
        // text stays text where no date is around
        assert_eq!(per_row("if(region = '2026-01-01', 1, 0)", Dialect::Postgres).unwrap().1, Type::Number);
        assert_eq!(per_row("to_text(sold_on)", Dialect::Postgres).unwrap().1, Type::Text);
    }

    #[test]
    fn spread_and_more_totals_with_a_condition() {
        assert_eq!(sql("stddev(amount)").unwrap(), "STDDEV(\"amount\")");
        assert_eq!(sql("variance(amount * qty)").unwrap(), "VARIANCE((\"amount\" * \"qty\"))");
        assert_eq!(sql("min_if(amount, returned)").unwrap(), "MIN(CASE WHEN \"returned\" THEN \"amount\" END)");
        assert_eq!(sql("max_if(amount, qty > 1)").unwrap(), "MAX(CASE WHEN (\"qty\" > 1) THEN \"amount\" END)");
        assert_eq!(
            sql("count_distinct_if(store, region = 'EMEA')").unwrap(),
            "COUNT_DISTINCT(CASE WHEN (\"region\" = 'EMEA') THEN \"store\" END)"
        );
        // a distinct count of text, a minimum of dates
        assert!(sql("count_distinct_if(store, returned)").is_ok());
        assert!(per_row("stddev(amount)", Dialect::Postgres).unwrap_err().message.contains("on totals"));
    }

    #[test]
    fn dates_as_text_and_fiscal_years() {
        let bq = |e| per_row(e, Dialect::BigQuery).unwrap().0;
        assert_eq!(bq("format_date(sold_on, 'YYYY-MM')"), "FORMAT_DATE('%Y-%m', \"sold_on\")");
        assert_eq!(bq("format_date(sold_at, 'Dy HH:MI')"), "FORMAT_TIMESTAMP('%a %H:%M', \"sold_at\", 'UTC')");
        assert_eq!(bq("format_date(naive, 'DD Mon')"), "FORMAT_DATETIME('%d %b', \"naive\")");
        assert_eq!(
            per_row("format_date(sold_on, 'YYYY-MM')", Dialect::DuckDb).unwrap().0,
            "STRFTIME(\"sold_on\", '%Y-%m')"
        );
        assert_eq!(
            per_row("format_date(sold_on, 'YYYY-MM')", Dialect::Postgres).unwrap().0,
            "TO_CHAR(\"sold_on\", 'YYYY-MM')"
        );
        let pg = |e| per_row(e, Dialect::Postgres).unwrap_err().message;
        assert!(
            pg("format_date(sold_on, 'YYYY Q')").contains("YYYY, MM, DD"),
            "{}",
            pg("format_date(sold_on, 'YYYY Q')")
        );
        assert!(pg("format_date(sold_on, store)").contains("in quotes"));
        assert!(pg("format_date(sold_on, 'DD HH')").contains("needs a time"));
        assert!(pg("fiscal_year(sold_on, 13)").contains("1 to 12"));
        assert!(pg("fiscal_quarter(sold_on)").contains("fiscal_quarter(…) takes"));
        assert_eq!(per_row("format_date(sold_on, 'Mon')", Dialect::Postgres).unwrap().1, Type::Text);
        assert_eq!(per_row("fiscal_year(sold_on, 7)", Dialect::Postgres).unwrap().1, Type::Number);
    }
}
