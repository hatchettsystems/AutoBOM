//! Prices, always shown as dollars: ".62" and "0.62" both become "$0.62". Same rules as the BladeDocs website.

use regex::Regex;
use std::cmp::Ordering;
use std::sync::OnceLock;

fn plain_amount() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^([^\d.,\-\s]{0,3})\s*(-?)\s*((?:\d{1,3}(?:,\d{3})+|\d+)?(?:\.\d+)?)$").unwrap())
}

/// Only plain amounts are touched ("$12", "12.5", ".62", "1,200"). Anything with words in it is left alone.
pub fn norm_cost(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() { return String::new(); }
    let m = match plain_amount().captures(t) { Some(m) => m, None => return t.to_string() };
    let digits = &m[3];
    if !digits.chars().any(|c| c.is_ascii_digit()) { return t.to_string(); }
    let sym = if m[1].is_empty() { "$" } else { &m[1] };
    let num: f64 = match digits.replace(',', "").parse() { Ok(n) => n, Err(_) => return t.to_string() };
    let neg = if m[2].is_empty() { "" } else { "-" };
    // cents are always shown; tiny unit prices (screws, washers) may keep up to four decimals: $0.035
    let dec = digits.split('.').nth(1).unwrap_or("");
    let keep = if num < 1.0 && dec.len() > 2 { dec.trim_end_matches('0').len().min(4).max(2) } else { 2 };
    let fixed = format!("{:.*}", keep, num);
    let (ip, fp) = fixed.split_once('.').unwrap_or((&fixed, "00"));
    let mut grouped = String::new();
    for (i, c) in ip.chars().enumerate() {
        if i > 0 && (ip.len() - i) % 3 == 0 { grouped.push(','); }
        grouped.push(c);
    }
    format!("{neg}{sym}{grouped}.{fp}")
}

/// The number in a price. ".62" is 0.62, not 62.
pub fn parse_cost(v: &str) -> Option<f64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"-?(?:\d+(?:\.\d+)?|\.\d+)").unwrap());
    let t = v.replace(',', "");
    re.find(&t).and_then(|m| m.as_str().parse().ok())
}

pub fn cost_symbol(v: &str) -> String {
    let t = v.trim();
    let s: String = t.chars().take_while(|c| !c.is_ascii_digit() && !c.is_whitespace() && *c != '.' && *c != '-').collect();
    s
}

pub fn fmt_money(n: f64, sym: &str) -> String {
    let sym = if sym.is_empty() { "$" } else { sym };
    let fixed = format!("{:.2}", n.abs());
    let (ip, fp) = fixed.split_once('.').unwrap();
    let mut g = String::new();
    for (i, c) in ip.chars().enumerate() {
        if i > 0 && (ip.len() - i) % 3 == 0 { g.push(','); }
        g.push(c);
    }
    format!("{}{sym}{g}.{fp}", if n < 0.0 { "-" } else { "" })
}

/// Alphabetical, ignoring case, with numbers compared as numbers ("Bearing 2" before "bearing 10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (mut i, mut j) = (0, 0);
    while i < ac.len() && j < bc.len() {
        if ac[i].is_ascii_digit() && bc[j].is_ascii_digit() {
            let si = i; while i < ac.len() && ac[i].is_ascii_digit() { i += 1; }
            let sj = j; while j < bc.len() && bc[j].is_ascii_digit() { j += 1; }
            let na: String = ac[si..i].iter().collect::<String>().trim_start_matches('0').to_string();
            let nb: String = bc[sj..j].iter().collect::<String>().trim_start_matches('0').to_string();
            let o = na.len().cmp(&nb.len()).then_with(|| na.cmp(&nb));
            if o != Ordering::Equal { return o; }
        } else {
            let (x, y) = (ac[i].to_lowercase().next().unwrap_or(ac[i]), bc[j].to_lowercase().next().unwrap_or(bc[j]));
            if x != y { return x.cmp(&y); }
            i += 1; j += 1;
        }
    }
    (ac.len() - i).cmp(&(bc.len() - j)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollars() {
        let cases = [(".62", "$0.62"), ("0.62", "$0.62"), ("$.62", "$0.62"), ("1.5", "$1.50"), ("12", "$12.00"), ("$1,200", "$1,200.00"),
            ("1200", "$1,200.00"), ("$0.035", "$0.035"), ("0.0125", "$0.0125"), ("$12 per pack", "$12 per pack"), ("free", "free"), ("", ""),
            ("€3.4", "€3.40"), ("-2", "-$2.00"), ("$ 4.5", "$4.50"), ("62", "$62.00"), ("0.1", "$0.10"), ("1,234.567", "$1,234.57"), (".5", "$0.50")];
        for (i, o) in cases { assert_eq!(norm_cost(i), o, "{i}"); }
    }

    #[test]
    fn reads_prices() {
        assert_eq!(parse_cost(".62"), Some(0.62));
        assert_eq!(parse_cost("$.62"), Some(0.62));
        assert_eq!(parse_cost("$1,200.50"), Some(1200.5));
        assert_eq!(parse_cost("free"), None);
        assert_eq!(parse_cost("€3.4"), Some(3.4));
        assert_eq!(fmt_money(220.5, "$"), "$220.50");
        assert_eq!(fmt_money(1234567.0, ""), "$1,234,567.00");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["Washer", "bearing 10", "Bearing 2", "Zip tie", "Belt", "Screw", "Nut"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["Bearing 2", "bearing 10", "Belt", "Nut", "Screw", "Washer", "Zip tie"]);
    }
}
