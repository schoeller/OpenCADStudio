/// Math expression evaluator — recursive-descent parser.
///
/// Supports operators: `+ - * / ^ %` with standard precedence.
/// Supports functions: `abs sin cos tan sqrt ceil floor round log exp`
/// Supports coord functions: `PtDist PtAngle`
/// Supports constant: `pi`
/// Case-insensitive for function names and constant.

// ── Token ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    LParen,
    RParen,
    Comma,
}

// ── Tokenizer ───────────────────────────────────────────────────────

fn tokenize(input: &str) -> Result<Vec<Token>, ()> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        match ch {
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            ',' => tokens.push(Token::Comma),
            '+' => tokens.push(Token::Plus),
            '-' => tokens.push(Token::Minus),
            '*' => tokens.push(Token::Star),
            '/' => tokens.push(Token::Slash),
            '^' => tokens.push(Token::Caret),
            '%' => tokens.push(Token::Percent),
            c if c.is_ascii_digit() || (c == '.' && chars.peek().map_or(false, |c| c.is_ascii_digit())) => {
                let mut num_str = String::new();
                if c == '.' {
                    num_str.push('.');
                } else {
                    num_str.push(c);
                }
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        num_str.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match num_str.parse::<f64>() {
                    Ok(v) => tokens.push(Token::Number(v)),
                    Err(_) => return Err(()),
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut ident = String::new();
                ident.push(c.to_ascii_lowercase());
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        ident.push(c.to_ascii_lowercase());
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Ident(ident));
            }
            _ => return Err(()),
        }
    }

    Ok(tokens)
}

// ── Parser ──────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

/// Maximum recursion depth for the recursive-descent parser. Deeply nested
/// expressions (e.g. thousands of parentheses) otherwise overflow the stack.
const MAX_RECURSION: usize = 256;

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect_paren(&mut self) -> Result<(), ()> {
        match self.advance() {
            Some(Token::LParen) => Ok(()),
            _ => Err(()),
        }
    }

    fn expect_rparen(&mut self) -> Result<(), ()> {
        match self.advance() {
            Some(Token::RParen) => Ok(()),
            _ => Err(()),
        }
    }

    // expr → sum
    fn parse_expr(&mut self, depth: usize) -> Result<f64, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        self.parse_sum(depth + 1)
    }

    // sum → product (('+' | '-') product)*
    fn parse_sum(&mut self, depth: usize) -> Result<f64, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        let mut left = self.parse_product(depth + 1)?;
        loop {
            let op = match self.peek().cloned() {
                Some(Token::Plus) => Some("+"),
                Some(Token::Minus) => Some("-"),
                _ => None,
            };
            match op {
                Some("+") => {
                    self.advance();
                    let right = self.parse_product(depth + 1)?;
                    left = left + right;
                }
                Some("-") => {
                    self.advance();
                    let right = self.parse_product(depth + 1)?;
                    left = left - right;
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // product → power (('*' | '/' | '%') power)*
    fn parse_product(&mut self, depth: usize) -> Result<f64, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        let mut left = self.parse_power(depth + 1)?;
        loop {
            let op = match self.peek().cloned() {
                Some(Token::Star) => Some("*"),
                Some(Token::Slash) => Some("/"),
                Some(Token::Percent) => Some("%"),
                _ => None,
            };
            match op {
                Some("*") => {
                    self.advance();
                    let right = self.parse_power(depth + 1)?;
                    left = left * right;
                }
                Some("/") => {
                    self.advance();
                    let right = self.parse_power(depth + 1)?;
                    if right == 0.0 {
                        return Ok(if left > 0.0 { f64::INFINITY } else if left < 0.0 { f64::NEG_INFINITY } else { f64::NAN });
                    }
                    left = left / right;
                }
                Some("%") => {
                    self.advance();
                    let right = self.parse_power(depth + 1)?;
                    left = left % right;
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // power → unary ('^' unary)?
    fn parse_power(&mut self, depth: usize) -> Result<f64, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        let base = self.parse_unary(depth + 1)?;
        if let Some(Token::Caret) = self.peek() {
            self.advance();
            let exp = self.parse_unary(depth + 1)?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    // unary → ('-' | '+') unary | atom
    fn parse_unary(&mut self, depth: usize) -> Result<f64, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        if let Some(Token::Minus) = self.peek() {
            self.advance();
            return Ok(-self.parse_unary(depth + 1)?);
        }
        if let Some(Token::Plus) = self.peek() {
            self.advance();
            return Ok(self.parse_unary(depth + 1)?);
        }
        self.parse_atom(depth + 1)
    }

    // atom → NUMBER | NAME | NAME '(' args ')' | 'pi'
    fn parse_atom(&mut self, depth: usize) -> Result<f64, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        let next = match self.peek() {
            Some(t) => t.clone(),
            None => return Err(()),
        };
        match next {
            Token::LParen => {
                self.advance();
                let result = self.parse_expr(depth + 1)?;
                self.expect_rparen()?;
                Ok(result)
            }
            Token::Number(v) => {
                self.advance();
                Ok(v)
            }
            Token::Ident(name) => {
                self.advance();
                // Check for function call
                if let Some(Token::LParen) = self.peek() {
                    self.expect_paren()?;
                    let args = self.parse_args(depth + 1)?;
                    self.expect_rparen()?;
                    return Self::call_function(&name, &args);
                }
                // Check for constant
                if name == "pi" {
                    Ok(std::f64::consts::PI)
                } else {
                    Err(())
                }
            }
            _ => Err(()),
        }
    }

    fn parse_args(&mut self, depth: usize) -> Result<Vec<f64>, ()> {
        if depth > MAX_RECURSION {
            return Err(());
        }
        let mut args = Vec::new();
        if matches!(self.peek(), Some(Token::RParen)) {
            return Ok(args);
        }
        args.push(self.parse_expr(depth + 1)?);
        loop {
            if matches!(self.peek(), Some(Token::RParen)) {
                break;
            }
            if let Some(Token::Comma) = self.peek() {
                self.advance();
                args.push(self.parse_expr(depth + 1)?);
            } else {
                break;
            }
        }
        Ok(args)
    }

    fn call_function(name: &str, args: &[f64]) -> Result<f64, ()> {
        let n = args.len();
        match name {
            "abs" => {
                if n != 1 { return Err(()); }
                Ok(args[0].abs())
            }
            "sin" => {
                if n != 1 { return Err(()); }
                Ok(args[0].sin())
            }
            "cos" => {
                if n != 1 { return Err(()); }
                Ok(args[0].cos())
            }
            "tan" => {
                if n != 1 { return Err(()); }
                Ok(args[0].tan())
            }
            "sqrt" => {
                if n != 1 { return Err(()); }
                Ok(args[0].sqrt().max(0.0))
            }
            "ceil" => {
                if n != 1 { return Err(()); }
                Ok(args[0].ceil())
            }
            "floor" => {
                if n != 1 { return Err(()); }
                Ok(args[0].floor())
            }
            "round" => {
                if n != 1 { return Err(()); }
                Ok(args[0].round())
            }
            "log" => {
                if n != 1 { return Err(()); }
                if args[0] <= 0.0 { return Err(()); }
                Ok(args[0].ln())
            }
            "exp" => {
                if n != 1 { return Err(()); }
                Ok(args[0].exp())
            }
            "atand" => {
                if n == 1 {
                    Ok(args[0].to_degrees())
                } else if n == 2 {
                    Ok((args[1] / args[0]).atan().to_degrees())
                } else {
                    Err(())
                }
            }
            "ptdist" => {
                if n != 4 { return Err(()); }
                let dx = args[2] - args[0];
                let dy = args[3] - args[1];
                Ok((dx * dx + dy * dy).sqrt())
            }
            "ptangle" => {
                if n != 4 { return Err(()); }
                let dx = args[2] - args[0];
                let dy = args[3] - args[1];
                Ok(dy.atan2(dx))
            }
            _ => Err(()),
        }
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Evaluate a string as an f64 expression.
/// Returns None on parse error.
pub fn eval_number(input: &str) -> Option<f64> {
    let tokens = tokenize(input.trim()).ok()?;
    let mut parser = Parser::new(tokens);
    let result = parser.parse_expr(0).ok()?;
    if parser.pos != parser.tokens.len() {
        return None;
    }
    Some(result)
}

/// Evaluate a string as a number expression, returning the result as a string.
///
/// Plain numbers pass through unchanged. The entire string is evaluated if it
/// forms a valid expression.  When the input contains a mix of expressions and
/// non-expression characters (commas, `@`, `#`, letters, etc.), the function
/// scans character-by-character, finds maximal contiguous substrings that parse
/// as valid expressions, evaluates each independently, and leaves everything
/// else as-is.
///
/// # Examples
///
/// ```ignore
/// // eval_to_string("10-5,0") == "5,0"
/// // eval_to_string("@10-5,0") == "@5,0"
/// // eval_to_string("def10*10abc") == "def100abc"
/// // eval_to_string("50") == "50"
/// // eval_to_string("10*5") == "50"
/// ```
pub fn eval_to_string(input: &str) -> String {
    let trimmed = input.trim();
    // Fast path: plain number or whole-string expression
    if trimmed.parse::<f64>().is_ok() {
        return trimmed.to_string();
    }
    if let Some(v) = eval_number(trimmed) {
        return format!("{}", v);
    }

    // Fallback: scan character-by-character, finding maximal expression substrings
    let chars: Vec<char> = trimmed.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        let mut found = false;
        // Try longest substring first (greedy)
        for end in (i + 1..=len).rev() {
            let sub: String = chars[i..end].iter().collect();
            if let Some(v) = eval_number(&sub) {
                result.push_str(&format!("{}", v));
                i = end;
                found = true;
                break;
            }
        }
        if !found {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_number_passthrough() {
        assert_eq!(eval_to_string("50"), "50");
        assert_eq!(eval_to_string("-3.14"), "-3.14");
        assert_eq!(eval_to_string("0"), "0");
        assert_eq!(eval_to_string("42.0"), "42.0");
    }

    #[test]
    fn arithmetic() {
        assert_eq!(eval_to_string("10*5"), "50");
        assert_eq!(eval_to_string("10 + 20"), "30");
        assert_eq!(eval_to_string("100/4"), "25");
        assert_eq!(eval_to_string("10-3"), "7");
        assert_eq!(eval_to_string("2^3"), "8");
        assert_eq!(eval_to_string("10%3"), "1");
        assert_eq!(eval_to_string("100 * 2 + 50"), "250");
    }

    #[test]
    fn precedence() {
        assert_eq!(eval_to_string("2+3*4"), "14");
        assert_eq!(eval_to_string("(2+3)*4"), "20");
        assert_eq!(eval_to_string("2^3*4"), "32");
        assert_eq!(eval_to_string("10/2+3"), "8");
        assert_eq!(eval_to_string("-2+4"), "2");
        assert_eq!(eval_to_string("3+4*5-2"), "21");
        assert_eq!(eval_to_string("(1+2)*(3+4)"), "21");
    }

    #[test]
    fn math_functions() {
        assert_eq!(eval_to_string("sqrt(4)"), "2");
        assert_eq!(eval_to_string("abs(-5)"), "5");
        assert_eq!(eval_to_string("round(3.7)"), "4");
        assert_eq!(eval_to_string("ceil(3.1)"), "4");
        assert_eq!(eval_to_string("floor(3.9)"), "3");
        assert_eq!(eval_to_string("sin(0)"), "0");
        assert_eq!(eval_to_string("cos(0)"), "1");
        assert_eq!(eval_to_string("exp(0)"), "1");
        assert_eq!(eval_to_string("log(1)"), "0");
    }

    #[test]
    fn constants() {
        let pi: f64 = eval_to_string("pi").parse().unwrap();
        assert!((pi - std::f64::consts::PI).abs() < 1e-10);
    }

    #[test]
    fn coord_functions() {
        assert_eq!(eval_to_string("ptdist(0,0,3,4)"), "5");
        let angle: f64 = eval_to_string("ptangle(0,0,3,4)").parse().unwrap();
        assert!((angle - 0.9272952180016122).abs() < 1e-10);
    }

    #[test]
    fn nested() {
        assert_eq!(eval_to_string("sqrt(16)+abs(-3)"), "7");
        assert_eq!(eval_to_string("(2+3)*(4-1)"), "15");
        let v: f64 = eval_to_string("sqrt(2^2+2^2)").parse().unwrap();
        assert!((v - 2.8284271247461903).abs() < 1e-10);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(eval_to_string("SQRT(4)"), "2");
        assert_eq!(eval_to_string("Abs(-5)"), "5");
        assert_eq!(eval_to_string("Pi"), eval_to_string("pi"));
        assert_eq!(eval_to_string("PI"), eval_to_string("pi"));
        assert_eq!(eval_to_string("SIN(0)"), "0");
    }

    #[test]
    fn invalid_passthrough() {
        assert_eq!(eval_to_string(""), "");
        assert_eq!(eval_to_string("abc"), "abc");
        assert_eq!(eval_to_string("(5"), "(5");
        assert_eq!(eval_to_string("foo(1,2)"), "foo(1,2)");
    }

    #[test]
    fn eval_number_direct() {
        assert_eq!(eval_number("10*5"), Some(50.0));
        assert_eq!(eval_number("sqrt(4)"), Some(2.0));
        assert_eq!(eval_number("abc"), None);
        assert_eq!(eval_number("(5"), None);
        assert_eq!(eval_number(""), None);
    }

    #[test]
    fn division_by_zero() {
        assert_eq!(eval_number("10/0"), Some(f64::INFINITY));
        assert_eq!(eval_number("-5/0"), Some(f64::NEG_INFINITY));
    }

    #[test]
    fn nested_parens() {
        assert_eq!(eval_to_string("((2+3))"), "5");
        assert_eq!(eval_to_string("(((10)))"), "10");
    }

    #[test]
    fn unary_ops() {
        assert_eq!(eval_to_string("--5"), "5");
        assert_eq!(eval_to_string("-(-3)"), "3");
        assert_eq!(eval_to_string("+-5"), "-5");
    }

    #[test]
    fn trig_functions() {
        let result: f64 = eval_to_string("sin(pi/2)").parse().unwrap();
        assert!((result - 1.0).abs() < 1e-10);
    }

    #[test]
    fn complex_expr() {
        assert_eq!(eval_to_string("2*(3+4)-5/2"), "11.5");
        assert_eq!(eval_to_string("sqrt(3^2+4^2)"), "5");
    }

    // ── Embedded expression tests ─────────────────────────────────────

    #[test]
    fn embedded_expr_in_coordinate() {
        assert_eq!(eval_to_string("10-5,0"), "5,0");
        assert_eq!(eval_to_string("10+5,20-3"), "15,17");
        assert_eq!(eval_to_string("2*3,4-1"), "6,3");
    }

    #[test]
    fn embedded_expr_with_at_prefix() {
        assert_eq!(eval_to_string("@10-5,0"), "@5,0");
        assert_eq!(eval_to_string("@10+5,20-3"), "@15,17");
    }

    #[test]
    fn embedded_expr_with_hash_prefix() {
        assert_eq!(eval_to_string("#10-5,0"), "#5,0");
        assert_eq!(eval_to_string("#2*3,4+1"), "#6,5");
    }

    #[test]
    fn embedded_expr_with_surrounding_text() {
        assert_eq!(eval_to_string("def10*10abc"), "def100abc");
        assert_eq!(eval_to_string("x=5*3+end"), "x=15+end");
    }

    #[test]
    fn embedded_expr_in_parentheses() {
        assert_eq!(eval_to_string("(10+5),0"), "15,0");
        assert_eq!(eval_to_string("(2+3)*(4-1),0"), "15,0");
    }

    #[test]
    fn embedded_expr_with_functions() {
        assert_eq!(eval_to_string("sqrt(4),3"), "2,3");
        assert_eq!(eval_to_string("abs(-5),ceil(3.1)"), "5,4");
    }

    #[test]
    fn embedded_expr_no_evaluables() {
        assert_eq!(eval_to_string("abc,def"), "abc,def");
        assert_eq!(eval_to_string("@,,"), "@,,");
    }

    #[test]
    fn embedded_expr_multiple_sub_expressions() {
        assert_eq!(eval_to_string("1+2,3*4,5-6"), "3,12,-1");
        assert_eq!(eval_to_string("2*3+1,4/2-1"), "7,1");
    }

    #[test]
    fn embedded_expr_text_with_3_commas() {
        assert_eq!(eval_to_string("10-5,0,2+3,4"), "5,0,5,4");
        assert_eq!(eval_to_string("@1+1,2*2,3-1,5/5"), "@2,4,2,1");
    }

    #[test]
    fn embedded_expr_text_complex() {
        assert_eq!(eval_to_string("start2*3end,5+5"), "start6end,10");
        assert_eq!(eval_to_string("a10+5b,3*2,3@10-5@"), "a15b,6,3@5@");
        assert_eq!(eval_to_string("#10+10,#5+5"), "#20,#10");
        assert_eq!(eval_to_string("sqrt(9)+1,cos(0)+1"), "4,2");
    }

    #[test]
    fn deeply_nested_parens_returns_none() {
        let expr = "(".repeat(1000) + "1" + &")".repeat(1000);
        assert_eq!(eval_number(&expr), None, "deeply nested expression should be rejected");
    }

    #[test]
    fn moderately_nested_parens_still_evaluate() {
        let expr = "(".repeat(30) + "1" + &")".repeat(30);
        assert_eq!(eval_number(&expr), Some(1.0), "moderately nested expression should evaluate");
    }
}
