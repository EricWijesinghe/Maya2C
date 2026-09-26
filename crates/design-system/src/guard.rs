//! Security UX detectors for the send, swap and connect screens
//! (Master Prompt 29 §2).
//!
//! Each detector returns a verdict the screen turns into a warning; none of
//! them blocks on its own, because a false block is itself a dead end. They
//! are heuristics with stated rules, and the tests pin both the attacks they
//! catch and the lookalikes they deliberately do not flag.

/// Characters shown at each end of a truncated address. Address poisoning
/// works because wallets show `0x1234…abcd` and people check only that.
pub const POISON_MATCH: usize = 4;

fn norm_addr(a: &str) -> String {
    a.trim().trim_start_matches("0x").to_ascii_lowercase()
}

/// If `candidate` is not in the address book but matches an entry's first
/// and last [`POISON_MATCH`] characters, returns that entry: the pattern a
/// poisoning transfer plants in the user's history.
#[must_use]
pub fn poisoning<'a>(candidate: &str, book: &[&'a str]) -> Option<&'a str> {
    let c = norm_addr(candidate);
    if c.len() < 2 * POISON_MATCH || book.iter().any(|b| norm_addr(b) == c) {
        return None;
    }
    book.iter().copied().find(|b| {
        let b = norm_addr(b);
        b.len() == c.len()
            && b[..POISON_MATCH] == c[..POISON_MATCH]
            && b[b.len() - POISON_MATCH..] == c[c.len() - POISON_MATCH..]
    })
}

/// Maps look-alike characters to the Latin letter they imitate and drops
/// invisible ones, so `USDС` (Cyrillic С) and `U​SDC` (zero-width space)
/// both become `USDC`. Uppercases the result.
#[must_use]
pub fn skeleton(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}') && !c.is_whitespace()
        })
        .map(|c| match c {
            'а' | 'А' | 'α' | 'Α' => 'A',
            'В' | 'Β' | 'в' => 'B',
            'с' | 'С' | 'ϲ' => 'C',
            'е' | 'Е' | 'ε' | 'Ε' => 'E',
            'Н' | 'Η' | 'н' => 'H',
            'і' | 'І' | 'Ι' | 'ι' | 'l' | '1' | '|' => 'I',
            'К' | 'Κ' | 'к' | 'κ' => 'K',
            'М' | 'Μ' | 'м' => 'M',
            'Ν' | 'ν' => 'N',
            'о' | 'О' | 'ο' | 'Ο' | '0' => 'O',
            'р' | 'Р' | 'ρ' | 'Ρ' => 'P',
            'ѕ' | 'Ѕ' | '5' => 'S',
            'Т' | 'Τ' | 'т' | 'τ' => 'T',
            'у' | 'У' | 'Υ' | 'υ' => 'Y',
            'х' | 'Х' | 'Χ' | 'χ' => 'X',
            'Ζ' => 'Z',
            c => c.to_ascii_uppercase(),
        })
        .collect()
}

/// A token as the wallet sees it: its displayed symbol and its contract id.
#[derive(Clone, Copy, Debug)]
pub struct Token<'a> {
    /// Displayed symbol.
    pub symbol: &'a str,
    /// Contract id; the only thing that identifies a token.
    pub contract: &'a str,
}

/// Verdict on a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenVerdict<'a> {
    /// The contract is on the verified list.
    Verified,
    /// Its symbol reads as a verified token's, but the contract differs.
    Impersonates(&'a str),
    /// Not verified, and not imitating one.
    Unknown,
}

/// Classifies `t` against the verified list. A symbol is never identity: an
/// exact `USDC` on the wrong contract is an impersonation too.
#[must_use]
pub fn token<'a>(t: Token<'_>, verified: &[Token<'a>]) -> TokenVerdict<'a> {
    if verified
        .iter()
        .any(|v| v.contract.eq_ignore_ascii_case(t.contract))
    {
        return TokenVerdict::Verified;
    }
    let sk = skeleton(t.symbol);
    verified
        .iter()
        .find(|v| skeleton(v.symbol) == sk)
        .map_or(TokenVerdict::Unknown, |v| {
            TokenVerdict::Impersonates(v.symbol)
        })
}

/// Verdict on a site asking to connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainVerdict<'a> {
    /// On the allow list, or a subdomain of an entry.
    Trusted,
    /// Internationalized (`xn--`) label: shown decoded, with a warning.
    Punycode,
    /// Reads like an allowed domain but is not it.
    LookAlike(&'a str),
    /// Nothing known either way.
    Unknown,
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Classifies a domain against the allow list. Rules, in order: exact or
/// subdomain match is trusted; any `xn--` label is flagged; an allowed
/// domain used as a prefix label (`maya2c.io.evil.com`), a homoglyph
/// skeleton match, or an edit distance of 1 or 2 is a look-alike.
#[must_use]
pub fn domain<'a>(host: &str, allow: &[&'a str]) -> DomainVerdict<'a> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if allow
        .iter()
        .any(|a| h == *a || h.ends_with(&format!(".{a}")))
    {
        return DomainVerdict::Trusted;
    }
    if h.split('.').any(|l| l.starts_with("xn--")) {
        return DomainVerdict::Punycode;
    }
    allow
        .iter()
        .copied()
        .find(|a| {
            h.starts_with(&format!("{a}."))
                || skeleton(&h) == skeleton(a)
                || edit_distance(&h, a) <= 2
        })
        .map_or(DomainVerdict::Unknown, DomainVerdict::LookAlike)
}
