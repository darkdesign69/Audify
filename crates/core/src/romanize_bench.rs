//! Romanization benchmark (stream: bench2, docs/bench/romanize.md): `romanize_line` against
//! hand-made standard readings of real lyric lines: modified Hepburn for Japanese (long vowels as
//! the kana spell them, particles wa/o/e), Revised Romanization with its sound changes for Korean,
//! Hanyu Pinyin with tone marks for Mandarin, simplified BGN/PCGN for Russian.
//!
//! `tests/fixtures/romanize/set.tsv`: `lang<TAB>line<TAB>reference`. The score is the character
//! error rate with case, punctuation and spaces ignored (word breaks are a style choice); Japanese
//! also folds long vowels (ou/ō/oo → o) so Hepburn variants count as equal.
//!
//! - `fixture_bench` (default suite, offline): holds every language's CER at or under the
//!   benchmarked level. `ROMANIZE_BENCH_OUT=<file>` writes our outputs for the Python scorer in
//!   `docs/bench/romanize-scripts/`; `ROMANIZE_BENCH_VERBOSE=1` prints the misses.

use super::romanize_line;
use unicode_normalization::UnicodeNormalization;

const SET: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/romanize/set.tsv"));

pub fn rows() -> Vec<(&'static str, &'static str, &'static str)> {
    SET.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut f = l.split('\t');
            (f.next().unwrap(), f.next().unwrap(), f.next().unwrap())
        })
        .collect()
}

/// Lowercase, letters and digits only (spaces dropped), tone marks kept unless `toneless`.
pub fn normalize(s: &str, lang: &str, toneless: bool) -> Vec<char> {
    let lower = s.to_lowercase();
    let mut t: String = if toneless || lang == "ja" {
        lower.nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect()
    } else {
        lower.nfc().collect()
    };
    if lang == "ja" {
        for (a, b) in [
            ("tch", "cch"),
            ("ou", "o"),
            ("oo", "o"),
            ("uu", "u"),
            ("aa", "a"),
            ("ee", "e"),
            ("ii", "i"),
        ] {
            t = t.replace(a, b);
        }
    }
    t.chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn edits(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Corpus CER per language: total edits / total reference characters.
fn cer(lang: &str, toneless: bool, verbose: bool) -> f64 {
    let (mut e, mut n) = (0, 0);
    for (l, line, want) in rows().into_iter().filter(|r| r.0 == lang) {
        let got = romanize_line(line, l == "ja");
        let (g, w) = (normalize(&got, l, toneless), normalize(want, l, toneless));
        let d = edits(&g, &w);
        if verbose && d > 0 {
            eprintln!("{l} {line}\n   got  {got}\n   want {want}");
        }
        e += d;
        n += w.len();
    }
    e as f64 / n as f64
}

#[test]
fn fixture_bench() {
    let verbose = std::env::var_os("ROMANIZE_BENCH_VERBOSE").is_some();
    if let Some(out) = std::env::var_os("ROMANIZE_BENCH_OUT") {
        let v: Vec<_> = rows()
            .into_iter()
            .map(|(l, line, _)| serde_json::json!({"lang": l, "text": line, "out": romanize_line(line, l == "ja")}))
            .collect();
        std::fs::write(out, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
    for (lang, toneless, max) in [
        ("ja", false, 0.010),
        ("ko", false, 0.005),
        ("zh", false, 0.020),
        ("zh", true, 0.008),
        ("ru", false, 0.005),
    ] {
        let c = cer(lang, toneless, verbose && !toneless);
        eprintln!(
            "romanize bench {lang}{}: CER {:.4}",
            if toneless { " (toneless)" } else { "" },
            c
        );
        assert!(c <= max, "{lang} CER {c:.4} > {max}");
    }
}
