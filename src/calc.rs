//! A small calculator: `2 * (3 + 4)`, `sqrt(2)^2`, `2pi`, `10 % 3`, ...

use std::f64::consts::{E, PI, TAU};

/// Longest input considered, and deepest nesting allowed. Both keep a pasted
/// wall of `-` or `(` from overflowing the stack.
const MAX_LEN: usize = 256;
const MAX_DEPTH: usize = 64;

/// Evaluates `input` if it looks like a math expression. Plain numbers and
/// anything that doesn't fully parse return `None`, so app names like
/// "7-Zip" never turn into calculator results.
pub fn evaluate(input: &str) -> Option<f64> {
    let s = input.trim();
    if s.len() > MAX_LEN {
        return None;
    }
    let has_operator = s.chars().any(|c| "+-*/%^()×÷".contains(c));
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let allowed = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || "+-*/%^().×÷".contains(c));
    if !has_operator || !has_digit || !allowed {
        return None;
    }

    let chars = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '×' => '*',
            '÷' => '/',
            c => c,
        })
        .collect();
    let mut parser = Parser {
        chars,
        pos: 0,
        depth: 0,
    };
    let value = parser.expr()?;
    (parser.pos == parser.chars.len() && value.is_finite()).then_some(value)
}

/// Formats a result for display and copying: integers without a decimal
/// point, and float noise like `0.30000000000000004` rounded away.
pub fn format(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    if value.abs() >= 1e15 || value.abs() < 1e-9 {
        return format!("{value:e}");
    }
    let s = format!("{value:.12}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Runs `f` one nesting level deeper, failing past `MAX_DEPTH`.
    fn nested(&mut self, f: impl FnOnce(&mut Self) -> Option<f64>) -> Option<f64> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let value = f(self);
        self.depth -= 1;
        value
    }

    fn expr(&mut self) -> Option<f64> {
        self.nested(Self::sum)
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            if self.eat('+') {
                value += self.term()?;
            } else if self.eat('-') {
                value -= self.term()?;
            } else {
                return Some(value);
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    value *= self.unary()?;
                }
                Some('/') => {
                    self.pos += 1;
                    value /= self.unary()?;
                }
                Some('%') => {
                    self.pos += 1;
                    value %= self.unary()?;
                }
                // Implicit multiplication: `2pi`, `3(4 + 1)`.
                Some(c) if c == '(' || c.is_ascii_alphabetic() => value *= self.power()?,
                _ => return Some(value),
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        self.nested(|p| {
            if p.eat('-') {
                Some(-p.unary()?)
            } else if p.eat('+') {
                p.unary()
            } else {
                p.power()
            }
        })
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if self.eat('^') {
            Some(base.powf(self.unary()?))
        } else {
            Some(base)
        }
    }

    fn primary(&mut self) -> Option<f64> {
        let c = self.peek()?;
        if c == '(' {
            self.pos += 1;
            let value = self.expr()?;
            // Tolerate a missing `)` at the very end while the user is typing.
            if !self.eat(')') && self.peek().is_some() {
                return None;
            }
            return Some(value);
        }
        if c.is_ascii_digit() || c == '.' {
            let start = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
                self.pos += 1;
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            return text.parse().ok();
        }
        if c.is_ascii_alphabetic() {
            let start = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric()) {
                self.pos += 1;
            }
            let name: String = self.chars[start..self.pos].iter().collect();
            return match name.to_ascii_lowercase().as_str() {
                "pi" => Some(PI),
                "e" => Some(E),
                "tau" => Some(TAU),
                func => {
                    if !self.eat('(') {
                        return None;
                    }
                    let arg = self.expr()?;
                    if !self.eat(')') && self.peek().is_some() {
                        return None;
                    }
                    apply(func, arg)
                }
            };
        }
        None
    }
}

fn apply(func: &str, x: f64) -> Option<f64> {
    Some(match func {
        "sqrt" => x.sqrt(),
        "cbrt" => x.cbrt(),
        "abs" => x.abs(),
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "asin" => x.asin(),
        "acos" => x.acos(),
        "atan" => x.atan(),
        "ln" => x.ln(),
        "log" => x.log10(),
        "log2" => x.log2(),
        "exp" => x.exp(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "round" => x.round(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(s: &str) -> Option<String> {
        evaluate(s).map(format)
    }

    #[test]
    fn arithmetic() {
        assert_eq!(calc("1 + 2 * 3").as_deref(), Some("7"));
        assert_eq!(calc("(1 + 2) * 3").as_deref(), Some("9"));
        assert_eq!(calc("10 / 4").as_deref(), Some("2.5"));
        assert_eq!(calc("10 % 3").as_deref(), Some("1"));
        assert_eq!(calc("0.1 + 0.2").as_deref(), Some("0.3"));
        assert_eq!(calc("2 ^ 3 ^ 2").as_deref(), Some("512"));
        assert_eq!(calc("-2 ^ 2").as_deref(), Some("-4"));
        assert_eq!(calc("3 × 4 ÷ 2").as_deref(), Some("6"));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(calc("sqrt(16) + 1").as_deref(), Some("5"));
        assert_eq!(calc("2pi + 1").as_deref(), Some("7.28318530718"));
        assert_eq!(calc("3(4 + 1)").as_deref(), Some("15"));
        assert_eq!(calc("(1 + 2").as_deref(), Some("3"));
    }

    #[test]
    fn rejects_non_math() {
        assert_eq!(calc("42"), None);
        assert_eq!(calc("7-zip"), None);
        assert_eq!(calc("c++"), None);
        assert_eq!(calc("notepad"), None);
        assert_eq!(calc("1 / 0"), None);
        assert_eq!(calc("Windows 11 - settings"), None);
    }

    #[test]
    fn deep_nesting_is_rejected_not_a_crash() {
        assert_eq!(calc(&format!("{}1", "-".repeat(200))), None);
        assert_eq!(calc(&format!("{}1{}", "(".repeat(100), ")".repeat(100))), None);
        assert_eq!(calc(&format!("{}1", "-".repeat(10_000))), None);
        assert_eq!(
            calc(&format!("{}1{}", "(".repeat(20), ")".repeat(20))).as_deref(),
            Some("1")
        );
    }
}
