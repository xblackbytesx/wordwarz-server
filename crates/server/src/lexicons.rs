//! The word lists, embedded at compile time from `lexicons/` (AGENTS.md
//! decision 4) and loaded once.

use std::sync::{Arc, LazyLock};

use wordwarz_core::lexicon::{Dictionary, Language, Lexicon};

static NL: &[u8] = include_bytes!("../../../lexicons/nl.fst");
static EN: &[u8] = include_bytes!("../../../lexicons/en.fst");

struct Dictionaries {
    dutch: Dictionary,
    english: Dictionary,
    both: Dictionary,
}

static DICTS: LazyLock<Dictionaries> = LazyLock::new(|| {
    let nl = Arc::new(Lexicon::from_bytes(NL.to_vec()).expect("embedded nl.fst"));
    let en = Arc::new(Lexicon::from_bytes(EN.to_vec()).expect("embedded en.fst"));
    Dictionaries {
        dutch: Dictionary::single(nl.clone()),
        english: Dictionary::single(en.clone()),
        both: Dictionary::new(vec![nl, en]),
    }
});

pub fn dictionary(language: Language) -> &'static Dictionary {
    match language {
        Language::Dutch => &DICTS.dutch,
        Language::English => &DICTS.english,
        Language::Bilingual => &DICTS.both,
    }
}

/// Loads both lists now, so the first move after a start is not the slow one.
pub fn warm() {
    LazyLock::force(&DICTS);
}
