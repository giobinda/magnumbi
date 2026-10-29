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
pub const FUNCTIONS: [&str; 31] = [
    "if", "coalesce", "round", "abs", "least", "greatest", "in", "case", // numbers, logic
    "lower", "upper", "concat", "contains", "starts_with", "ends_with", "length", "trim", "left", "right", "replace",
    "year", "quarter", "month", "week", "day", "weekday", "hour", "today", "days_between", // dates
    "date_trunc", "date_add", "date_diff",
];
/// The parts of a date the functions of that name give, as numbers.
const DATE_PARTS: [&str; 7] = ["year", "quarter", "month", "week", "day", "weekday", "hour"];
/// Calculations on a chart's results (window functions, run on ScramDB: results.rs).
pub const RESULT_FUNCTIONS: [&str; 5] = ["running_sum", "moving_avg", "previous", "rank", "percent_of_total"];
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
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
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
                if !FUNCTIONS.contains(&lower.as_str()) && !RESULT_FUNCTIONS.contains(&lower.as_str()) {
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

fn walk(n: &Node, visit: &mut impl FnMut(&Node)) {
    visit(n);
    match n {
        Node::Neg(x) | Node::Not(x) => walk(x, visit),
        Node::Binary { left, right, .. } => {
            walk(left, visit);
            walk(right, visit);
        }
        Node::Call { args, .. } => args.iter().for_each(|a| walk(a, visit)),
        _ => {}
    }
}

/// The rows a calculation on results works over: `along` (the first date dimension, or the first
/// dimension) for running totals and the like, separately for each value of the `across`
/// dimensions; rank and % of total go across, within each value along. SQL column references.
pub struct Window {
    pub along: String,
    pub across: Vec<String>,
}

/// A calculation on a chart's results, as ScramDB SQL over the result's work table.
pub fn compile_on_results(
    expr: &str,
    window: &Window,
    resolve: &mut impl FnMut(&str) -> Result<(String, Type), String>,
) -> Result<(String, Type), FormulaError> {
    let node = parse(expr)?;
    let mut c = Compiler { dl: Dialect::Postgres, resolve, window: Some(window), inside: false };
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

/// The SQL of a formula, and what it gives. `resolve` gives the SQL of a field or aggregation and
/// its type, or why there's no such thing (it decides which fields a formula may use: measures
/// for a formula on totals, columns for one per row).
pub fn compile(
    expr: &str,
    dl: Dialect,
    resolve: &mut impl FnMut(&str) -> Result<(String, Type), String>,
) -> Result<(String, Type), FormulaError> {
    let node = parse(expr)?;
    let mut c = Compiler { dl, resolve, window: None, inside: false };
    let (sql, kind) = c.node(&node)?;
    let t = match kind {
        Kind::Text => Type::Text,
        Kind::Condition => Type::Condition,
        Kind::Date => Type::Date,
        Kind::Time => Type::Time,
        Kind::DateTime => Type::DateTime,
        Kind::Number | Kind::Any => Type::Number,
    };
    Ok((sql, t))
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
        match self.node(n)? {
            (sql, k) if k.is_date() => Ok((sql, k)),
            _ => fail(at, "a date is expected here, such as a date field"),
        }
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
            Node::Field { name, at } => match (self.resolve)(name) {
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
                let ((l, a), (r, b)) = (self.node(left)?, self.node(right)?);
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
        let n = match (name, args) {
            ("moving_avg", [_, Node::Number(n)]) => match n.parse::<u32>() {
                Ok(n) if (1..=1000).contains(&n) => n,
                _ => return fail(at, "moving_avg(…)'s number of rows must be a whole number from 1 to 1000"),
            },
            ("moving_avg", _) => {
                return fail(at, "moving_avg(…) takes a number and how many rows: moving_avg(revenue, 3)");
            }
            (_, [_]) => 0,
            _ => return fail(at, format!("{name}(…) takes one number")),
        };
        self.inside = true;
        let x = self.number(&args[0], at)?;
        self.inside = false;
        let mut along = vec![];
        if !w.across.is_empty() {
            along.push(format!("PARTITION BY {}", w.across.join(", ")));
        }
        if !w.along.is_empty() {
            along.push(format!("ORDER BY {}", w.along));
        }
        let across =
            if w.across.is_empty() || w.along.is_empty() { String::new() } else { format!("PARTITION BY {}", w.along) };
        let frame =
            |from: &str| [along.join(" "), format!("ROWS BETWEEN {from} AND CURRENT ROW")].join(" ").trim().to_string();
        let sql = match name {
            "running_sum" => format!("SUM({x}) OVER ({})", frame("UNBOUNDED PRECEDING")),
            "moving_avg" => format!("AVG({x}) OVER ({})", frame(&format!("{} PRECEDING", n - 1))),
            "previous" => format!("LAG({x}) OVER ({})", along.join(" ")),
            "rank" => format!("RANK() OVER ({})", [across, format!("ORDER BY {x} DESC")].join(" ").trim()),
            _ => format!("(CAST({x} AS {}) / NULLIF(SUM({x}) OVER ({across}), 0))", self.dl.double()),
        };
        Ok((sql, Kind::Number))
    }

    fn call(&mut self, name: &str, args: &[Node], at: usize) -> Result<(String, Kind), FormulaError> {
        if RESULT_FUNCTIONS.contains(&name) {
            return self.on_results(name, args, at);
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

    /// A formula on totals: a number.
    fn sql(expr: &str) -> Result<String, FormulaError> {
        match compile(expr, Dialect::Postgres, &mut resolve)? {
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
        let w = Window { along: "\"c0\"".into(), across: vec!["\"c3\"".into()] };
        compile_on_results(expr, &w, &mut result).map(|(sql, _)| sql)
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
        // one dimension: nothing to partition by
        let w = Window { along: "\"c0\"".into(), across: vec![] };
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
    const DATES: [(&str, &str); 31] = [
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
        answer_queries(dl, &DATES, &mut date_columns(dl))
    }

    /// The text functions' answers, on a sale in 'EMEA' from the store 'Milano Centro'.
    const TEXTS: [(&str, &str); 26] = [
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
                other => Err(format!("unknown column {other:?}")),
            }
        }
    }

    pub(crate) fn text_queries(dl: Dialect) -> Vec<(String, &'static str, &'static str)> {
        answer_queries(dl, &TEXTS, &mut text_columns(dl))
    }

    /// Each formula as a query giving its answer as text (a date as its day).
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
                (format!("SELECT CAST({sql} AS {})", dl.text()), *expr, *want)
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
                assert_eq!(row.get::<_, String>(0), want, "{var}: {expr}: {sql}");
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
        let mut there: Vec<&str> = FUNCTIONS.iter().chain(&RESULT_FUNCTIONS).copied().collect();
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
}
