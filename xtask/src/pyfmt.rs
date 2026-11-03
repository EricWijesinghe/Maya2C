//! Output formatting that matches what the retired Python scripts printed.
//!
//! The generated files (`docs/benchmarks/*.csv`, `benchmarks/history.csv`,
//! the reports) are committed and diffed, so a formatter that spells `45.0` as
//! `45` or `1e-05` as `1e-5` is a diff nobody asked for. These helpers are the
//! three Python behaviours the ports relied on: `repr(float)`, `{:,.Nf}` and
//! `{:.3e}`, plus `csv.writer`'s `\r\n` terminator and minimal quoting.

/// Python's `f"{x:.{decimals}f}"`, with `inf` and `nan` spelled as Python does.
pub fn fixed(x: f64, decimals: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    format!("{x:.decimals$}")
}

/// Python's `f"{x:,.{decimals}f}"`: thousands separators in the integer part.
pub fn grouped(x: f64, decimals: usize) -> String {
    let body = fixed(x.abs(), decimals);
    if !x.is_finite() {
        return if x.is_sign_negative() {
            format!("-{body}")
        } else {
            body
        };
    }
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body.as_str(), None),
    };
    let mut out = String::new();
    if x.is_sign_negative() {
        out.push('-');
    }
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// Python's `f"{x:.{decimals}e}"`: the exponent has a sign and two digits at least.
pub fn exp(x: f64, decimals: usize) -> String {
    if !x.is_finite() {
        return fixed(x, decimals);
    }
    let rendered = format!("{x:.decimals$e}");
    let Some((mantissa, power)) = rendered.split_once('e') else {
        return rendered;
    };
    let power: i32 = power.parse().unwrap_or(0);
    let sign = if power < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", power.abs())
}

/// Python's `repr(float)`: the shortest round-trip digits, `.0` kept on whole
/// numbers, and exponent notation outside `1e-4 <= |x| < 1e16`.
pub fn repr(x: f64) -> String {
    if !x.is_finite() {
        return fixed(x, 0);
    }
    let sci = format!("{x:e}");
    let Some((mantissa, power)) = sci.split_once('e') else {
        return sci;
    };
    let Ok(exp) = power.parse::<i32>() else {
        return sci;
    };
    let (sign, unsigned) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits: String = unsigned.chars().filter(|c| *c != '.').collect();
    let body = if (-4..16).contains(&exp) {
        fixed_notation(&digits, exp)
    } else {
        let (head, tail) = digits.split_at(1);
        let dot = if tail.is_empty() { "" } else { "." };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{head}{dot}{tail}e{esign}{:02}", exp.abs())
    };
    format!("{sign}{body}")
}

/// Places the decimal point of `digits` (first digit at 10^`exp`).
fn fixed_notation(digits: &str, exp: i32) -> String {
    if exp < 0 {
        let zeros = "0".repeat(usize::try_from(-exp - 1).unwrap_or(0));
        return format!("0.{zeros}{digits}");
    }
    let int_len = usize::try_from(exp).unwrap_or(0) + 1;
    if digits.len() <= int_len {
        let zeros = "0".repeat(int_len - digits.len());
        return format!("{digits}{zeros}.0");
    }
    let (int, frac) = digits.split_at(int_len);
    format!("{int}.{frac}")
}

/// One `csv.writer` row: minimal quoting, `\r\n` terminator (what Python writes
/// with `newline=""`).
pub fn csv_row<S: AsRef<str>>(fields: &[S]) -> String {
    let mut line = String::new();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        let f = field.as_ref();
        if f.contains([',', '"', '\r', '\n']) {
            line.push('"');
            line.push_str(&f.replace('"', "\"\""));
            line.push('"');
        } else {
            line.push_str(f);
        }
    }
    line.push_str("\r\n");
    line
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{csv_row, exp, fixed, grouped, repr};

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr(45.0), "45.0");
        assert_eq!(repr(0.0001), "0.0001");
        assert_eq!(repr(0.00001), "1e-05");
        assert_eq!(repr(1234.5), "1234.5");
        assert_eq!(repr(1e16), "1e+16");
        assert_eq!(repr(-2.5e-7), "-2.5e-07");
        assert_eq!(repr(0.0), "0.0");
    }

    #[test]
    fn fixed_grouped_and_exp_match_python() {
        assert_eq!(fixed(2.0 / 3.0, 2), "0.67");
        assert_eq!(grouped(1_234_567.4, 0), "1,234,567");
        assert_eq!(grouped(-1234.56, 1), "-1,234.6");
        assert_eq!(exp(1234.5, 3), "1.234e+03");
        assert_eq!(exp(0.0, 3), "0.000e+00");
        assert_eq!(exp(0.000_012_3, 3), "1.230e-05");
    }

    #[test]
    fn csv_quotes_only_when_needed() {
        assert_eq!(csv_row(&["a", "b,c", "d\"e"]), "a,\"b,c\",\"d\"\"e\"\r\n");
    }
}
