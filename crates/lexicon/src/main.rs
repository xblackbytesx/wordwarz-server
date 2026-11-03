//! Builds the lexicons and derives the tile sets. Run through `make lexicons`,
//! which downloads the pinned sources (lexicons/SOURCES.md) first.
//!
//!   wordwarz-lexicon build <input.txt> <out.fst>
//!   wordwarz-lexicon tiles <lexicons dir> <tilesets.rs>

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use wordwarz_core::letters::{ALPHABET, Letter};
use wordwarz_core::lexicon::{Lexicon, normalize_word};
use wordwarz_core::tiles::TileSet;

/// Letter frequencies count words up to this length: the words people
/// actually play, so long Dutch compounds do not skew the bag.
const FREQ_MAX_LEN: usize = 8;

const BEGIN: &str = "// @generated begin\n";
const END: &str = "// @generated end\n";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["build", input, out] => build(Path::new(input), Path::new(out)),
        ["tiles", dir, target] => tiles(Path::new(dir), Path::new(target)),
        _ => bail!("usage: wordwarz-lexicon build <input.txt> <out.fst> | tiles <lexicons dir> <tilesets.rs>"),
    }
}

fn build(input: &Path, out: &Path) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    let total = text.lines().count();
    let mut words: Vec<String> = text.lines().filter_map(normalize_word).collect();
    words.sort();
    words.dedup();
    let set = fst::Set::from_iter(&words)?;
    let bytes = set.into_fst().into_inner();
    fs::write(out, &bytes).with_context(|| format!("writing {}", out.display()))?;
    println!("{}: {} lines -> {} words, {} KiB", out.display(), total, words.len(), bytes.len() / 1024);
    Ok(())
}

fn tiles(dir: &Path, target: &Path) -> Result<()> {
    let load = |name: &str| -> Result<Lexicon> {
        let p = dir.join(name);
        Lexicon::from_bytes(fs::read(&p).with_context(|| format!("reading {}", p.display()))?)
            .map_err(anyhow::Error::from)
    };
    let nl = load("nl.fst")?.letter_frequencies(FREQ_MAX_LEN);
    let en = load("en.fst")?.letter_frequencies(FREQ_MAX_LEN);
    let share = |f: &[f64; ALPHABET]| {
        let sum: f64 = f.iter().sum();
        f.map(|x| x / sum)
    };
    let (nl_s, en_s) = (share(&nl), share(&en));
    let mut both = [0.0; ALPHABET];
    for i in 0..ALPHABET {
        both[i] = (nl_s[i] + en_s[i]) / 2.0;
    }

    let mut block = String::from(BEGIN);
    for (name, freq) in [("DUTCH", nl_s), ("ENGLISH", en_s), ("BILINGUAL", both)] {
        let t = TileSet::derive(&freq);
        block.push_str(&format!("static {name}: TileSet = set(\n    {:?},\n    {:?},\n);\n", t.counts, t.values));
        let summary: Vec<String> =
            Letter::all().map(|l| format!("{}x{}={}", l, t.counts[l.index()], t.values[l.index()])).collect();
        println!("{name}: {}", summary.join(" "));
    }
    block.push_str(END);

    // The markers must be there exactly once, or nothing is written.
    let src = fs::read_to_string(target).with_context(|| format!("reading {}", target.display()))?;
    let (Some(b), Some(e)) = (src.find(BEGIN), src.find(END)) else {
        bail!("{} lacks the @generated markers", target.display());
    };
    if src.matches(BEGIN).count() != 1 || src.matches(END).count() != 1 || e < b {
        bail!("{} has misplaced @generated markers", target.display());
    }
    let new = format!("{}{}{}", &src[..b], block, &src[e + END.len()..]);
    fs::write(target, new)?;
    println!("wrote {}", target.display());
    Ok(())
}
