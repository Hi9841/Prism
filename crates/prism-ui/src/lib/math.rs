//! Safe math evaluation - a tiny recursive-descent parser.
//! No eval, no global state. Handles + - * / ^ % ( ) and common
//! functions/constants. Returns `None` when the expression isn't math.

use std::sync::LazyLock;

use regex::Regex;

use crate::js;

pub struct EvalResult {
    pub value: f64,
    pub expr: String,
}

fn func(name: &str) -> Option<fn(f64) -> f64> {
    Some(match name {
        "sqrt" => f64::sqrt,
        "abs" => f64::abs,
        "floor" => f64::floor,
        "ceil" => f64::ceil,
        "round" => js::round,
        "ln" => f64::ln,
        "log" => f64::log10,
        "sin" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "asin" => f64::asin,
        "acos" => f64::acos,
        "atan" => f64::atan,
        "exp" => f64::exp,
        _ => return None,
    })
}

fn constant(name: &str) -> Option<f64> {
    match name {
        "pi" => Some(std::f64::consts::PI),
        "e" => Some(std::f64::consts::E),
        "tau" => Some(std::f64::consts::PI * 2.0),
        _ => None,
    }
}

static SQRT_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"√([0-9.]+)").unwrap());
static HAS_CONSTANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^a-z])(?:pi|e|tau)(?:[^a-z]|$)").unwrap());
static KNOWN_MATH_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)(?:sqrt|abs|floor|ceil|round|ln|log|sin|cos|tan|asin|acos|atan|exp|pi|e|tau)(?-u:\b)")
        .unwrap()
});
static LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+(?:\.[0-9]+)?$").unwrap());

/// `/^[\d\s.+\-*/^%()a-z]+$/`
fn only_math_chars(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_digit() || c.is_ascii_lowercase() || js::is_whitespace(c) || ".+-*/^%()".contains(c)
        })
}

fn rewrite_symbols(s: &str) -> String {
    let s = s.replace('×', "*").replace('÷', "/");
    let s = SQRT_NUMBER.replace_all(&s, "sqrt($1)");
    s.replace('√', "sqrt(").replace('π', "pi")
}

fn normalize(input: &str) -> Option<String> {
    let s = js::lower(js::trim(input));
    let length = js::len(&s);
    if length == 0 || length > 120 {
        return None;
    }
    let s = rewrite_symbols(&s);
    if !only_math_chars(&s) {
        return None;
    }
    if !s.bytes().any(|b| b.is_ascii_digit()) && !HAS_CONSTANT.is_match(&s) {
        return None;
    }
    Some(s)
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Num(f64),
    Op(char),
    LParen,
    RParen,
    Ident(String),
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

type Parsed = Result<f64, ()>;

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn parse(&mut self) -> Parsed {
        let v = self.expr()?;
        if self.pos != self.tokens.len() {
            return Err(()); // trailing input
        }
        Ok(v)
    }

    fn expr(&mut self) -> Parsed {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(Token::Op(op @ ('+' | '-'))) => {
                    let op = *op;
                    self.next();
                    let rhs = self.term()?;
                    v = if op == '+' { v + rhs } else { v - rhs };
                }
                _ => return Ok(v),
            }
        }
    }

    fn term(&mut self) -> Parsed {
        let mut v = self.factor()?;
        loop {
            match self.peek() {
                Some(Token::Op(op @ ('*' | '/' | '%'))) => {
                    let op = *op;
                    self.next();
                    let rhs = self.factor()?;
                    v = match op {
                        '*' => v * rhs,
                        '/' => v / rhs,
                        _ => v % rhs,
                    };
                }
                _ => return Ok(v),
            }
        }
    }

    fn factor(&mut self) -> Parsed {
        if let Some(Token::Op(op @ ('-' | '+'))) = self.peek() {
            let op = *op;
            self.next();
            let v = self.factor()?;
            return Ok(if op == '-' { -v } else { v });
        }
        let v = self.primary()?;
        // Power binds tighter than everything: -2^2 == -(2^2)
        if let Some(Token::Op('^')) = self.peek() {
            self.next();
            let exponent = self.factor()?;
            return Ok(js::pow(v, exponent));
        }
        Ok(v)
    }

    fn primary(&mut self) -> Parsed {
        match self.next().ok_or(())? {
            Token::Num(v) => Ok(v),
            Token::LParen => {
                let v = self.expr()?;
                match self.next() {
                    Some(Token::RParen) => Ok(v),
                    _ => Err(()), // expected )
                }
            }
            Token::Ident(name) => {
                if let Some(value) = constant(&name) {
                    return Ok(value);
                }
                let f = func(&name).ok_or(())?;
                if self.next() != Some(Token::LParen) {
                    return Err(()); // expected ( after function
                }
                let arg = self.expr()?;
                if self.next() != Some(Token::RParen) {
                    return Err(()); // expected ) after function arg
                }
                Ok(f(arg))
            }
            _ => Err(()),
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<Token>, ()> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            // `Number(text)`: "1." and ".5" parse, "." and "1.2.3" are NaN.
            let num: f64 = text.parse().map_err(|_| ())?;
            if !num.is_finite() {
                return Err(()); // bad number
            }
            out.push(Token::Num(num));
            i = j;
            continue;
        }
        if c.is_ascii_lowercase() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_lowercase() {
                j += 1;
            }
            out.push(Token::Ident(chars[i..j].iter().collect()));
            i = j;
            continue;
        }
        if "+-*/^%".contains(c) {
            out.push(Token::Op(c));
            i += 1;
            continue;
        }
        match c {
            '(' => out.push(Token::LParen),
            ')' => out.push(Token::RParen),
            _ => return Err(()), // unexpected char
        }
        i += 1;
    }
    Ok(out)
}

/// Formats a result number: 6 significant digits, no trailing zeros.
pub fn format_number(n: f64) -> String {
    if !n.is_finite() {
        return "Undefined".into();
    }
    if js::is_integer(n) {
        return js::number_to_string(n);
    }
    js::number_to_string(js::round_to_precision(n, 6))
}

/// Returns the evaluation result if `input` parses as a math expression.
pub fn try_evaluate(input: &str) -> Option<EvalResult> {
    let s = normalize(input)?;
    let tokens = tokenize(&s).ok()?;
    let value = Parser { tokens, pos: 0 }.parse().ok()?;
    Some(EvalResult { value, expr: s.chars().filter(|c| !js::is_whitespace(*c)).collect() })
}

/// Returns true when a query is "math-like" enough to surface the calculator.
pub fn is_math_like(input: &str) -> bool {
    let s = js::lower(js::trim(input));
    let length = js::len(&s);
    if length == 0 || length > 120 {
        return false;
    }
    let s = rewrite_symbols(&s);
    if !only_math_chars(&s) {
        return false;
    }

    // Application names such as "vlc3" and "7zip" contain digits but have no
    // math syntax. Keep calculator discovery for literals and known parser
    // tokens while avoiding a parser attempt for ordinary names.
    let known_math_token = KNOWN_MATH_TOKEN.is_match(&s);
    let has_operator = s.chars().any(|c| "+-*/^%()".contains(c));
    let is_literal = LITERAL.is_match(&s);
    let has_digit = s.bytes().any(|b| b.is_ascii_digit());
    if !has_digit && !known_math_token {
        return false;
    }
    if !has_operator && !is_literal && !known_math_token {
        return false;
    }
    if s.bytes().any(|b| b.is_ascii_lowercase()) && !known_math_token {
        return false;
    }
    true
}

// PORT STATUS: confidence=high todos=0
