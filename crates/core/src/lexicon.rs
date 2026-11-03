//! Word lists as finite state transducers (the `fst` crate). The bytes come
//! from `lexicons/*.fst` (built by `wordwarz-lexicon`); the core never reads
//! files itself. A `Dictionary` is one lexicon or, for a bilingual game, the
//! union of two, walked side by side so the move generator sees one trie.

use std::sync::Arc;

use fst::raw::{CompiledAddr, Fst};
use serde::{Deserialize, Serialize};

use crate::letters::{Letter, word};

/// Shortest and longest words kept: one-letter words are never words in
/// this game, and no board is wider than 19.
pub const MIN_WORD: usize = 2;
pub const MAX_WORD: usize = 19;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "nl")]
    Dutch,
    #[serde(rename = "en")]
    English,
    /// Words count in Dutch or English (decision 8).
    #[serde(rename = "nl+en")]
    Bilingual,
}

/// Lexicon file names under `lexicons/`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LexiconId {
    Dutch,
    English,
}

impl LexiconId {
    pub fn file(self) -> &'static str {
        match self {
            LexiconId::Dutch => "nl.fst",
            LexiconId::English => "en.fst",
        }
    }
}

impl Language {
    pub fn lexicons(self) -> &'static [LexiconId] {
        match self {
            Language::Dutch => &[LexiconId::Dutch],
            Language::English => &[LexiconId::English],
            Language::Bilingual => &[LexiconId::Dutch, LexiconId::English],
        }
    }
}

/// Turns one line of a source word list into a lexicon entry, or rejects it.
/// Rejects proper nouns and abbreviations (any capital), words with digits,
/// hyphens, apostrophes, dots or spaces, and lengths outside 2..=19. Folds
/// diacritics (`ideeën` -> `ideeen`, `café` -> `cafe`).
pub fn normalize_word(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for c in raw.trim().chars() {
        let folded = match c {
            'a'..='z' => c,
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            'ç' => 'c',
            'ñ' => 'n',
            _ => return None,
        };
        out.push(folded);
    }
    (MIN_WORD..=MAX_WORD).contains(&out.len()).then_some(out)
}

#[derive(Debug, thiserror::Error)]
#[error("not a lexicon: {0}")]
pub struct LexiconError(String);

/// One word list.
pub struct Lexicon {
    fst: Fst<Vec<u8>>,
}

impl Lexicon {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Lexicon, LexiconError> {
        Fst::new(bytes).map(|fst| Lexicon { fst }).map_err(|e| LexiconError(e.to_string()))
    }

    /// Builds one in memory from words (any order, normalized or not).
    pub fn from_words<'a>(words: impl IntoIterator<Item = &'a str>) -> Lexicon {
        let mut v: Vec<String> = words.into_iter().filter_map(normalize_word).collect();
        v.sort();
        v.dedup();
        let set = fst::Set::from_iter(v).expect("sorted and deduplicated");
        Lexicon { fst: Fst::new(set.into_fst().into_inner()).expect("just built") }
    }

    pub fn len(&self) -> usize {
        self.fst.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fst.is_empty()
    }

    pub fn contains(&self, w: &[Letter]) -> bool {
        let bytes: Vec<u8> = w.iter().map(|l| l.byte()).collect();
        self.fst.contains_key(bytes)
    }

    /// Every word, in order, as lowercase bytes.
    pub fn for_each(&self, mut f: impl FnMut(&[u8])) {
        use fst::Streamer;
        let mut s = self.fst.stream();
        while let Some((k, _)) = s.next() {
            f(k);
        }
    }

    /// Letter occurrences per letter in words of up to `max_len` letters:
    /// the input to `TileSet::derive`.
    pub fn letter_frequencies(&self, max_len: usize) -> [f64; 26] {
        let mut freq = [0.0; 26];
        self.for_each(|w| {
            if w.len() <= max_len {
                for &b in w {
                    freq[(b - b'a') as usize] += 1.0;
                }
            }
        });
        freq
    }
}

/// A trie cursor: one fst node per lexicon in the dictionary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Node([Option<CompiledAddr>; 2]);

/// The words a game accepts: one lexicon, or two for a bilingual game.
#[derive(Clone)]
pub struct Dictionary {
    parts: Vec<Arc<Lexicon>>,
}

impl Dictionary {
    pub fn new(parts: Vec<Arc<Lexicon>>) -> Dictionary {
        assert!((1..=2).contains(&parts.len()), "a dictionary has one or two lexicons");
        Dictionary { parts }
    }

    pub fn single(l: Arc<Lexicon>) -> Dictionary {
        Dictionary::new(vec![l])
    }

    pub fn contains(&self, w: &[Letter]) -> bool {
        self.parts.iter().any(|p| p.contains(w))
    }

    /// Convenience for tests and tools: `"cat"` or `"CAT"`.
    pub fn contains_str(&self, s: &str) -> bool {
        word(s).is_some_and(|w| self.contains(&w))
    }

    pub fn root(&self) -> Node {
        let mut n = [None, None];
        for (i, p) in self.parts.iter().enumerate() {
            n[i] = Some(p.fst.root().addr());
        }
        Node(n)
    }

    pub fn step(&self, node: Node, l: Letter) -> Option<Node> {
        let mut next = [None, None];
        let mut any = false;
        for (i, p) in self.parts.iter().enumerate() {
            if let Some(addr) = node.0[i] {
                let n = p.fst.node(addr);
                if let Some(t) = n.find_input(l.byte()) {
                    next[i] = Some(n.transition(t).addr);
                    any = true;
                }
            }
        }
        any.then_some(Node(next))
    }

    pub fn is_word(&self, node: Node) -> bool {
        self.parts.iter().enumerate().any(|(i, p)| node.0[i].is_some_and(|a| p.fst.node(a).is_final()))
    }

    /// Walks a whole prefix from the root.
    pub fn walk(&self, letters: &[Letter]) -> Option<Node> {
        letters.iter().try_fold(self.root(), |n, &l| self.step(n, l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_plain_words_and_folds_accents() {
        assert_eq!(normalize_word("ideeën").as_deref(), Some("ideeen"));
        assert_eq!(normalize_word("café\n").as_deref(), Some("cafe"));
        assert_eq!(normalize_word("Kevin"), None);
        assert_eq!(normalize_word("NAVO-verband"), None);
        assert_eq!(normalize_word("filmstudio's"), None);
        assert_eq!(normalize_word("a"), None);
        assert_eq!(normalize_word("abcdefghijklmnopqrst"), None);
    }

    #[test]
    fn union_walks_both_lexicons() {
        let nl = Arc::new(Lexicon::from_words(["huis", "kat", "ij"]));
        let en = Arc::new(Lexicon::from_words(["house", "cat", "hui"]));
        let d = Dictionary::new(vec![nl, en]);
        for w in ["huis", "kat", "ij", "house", "cat"] {
            assert!(d.contains_str(w), "{w}");
            assert!(d.is_word(d.walk(&word(w).unwrap()).unwrap()), "{w}");
        }
        assert!(!d.contains_str("hou"));
        let hui = d.walk(&word("hui").unwrap()).unwrap();
        assert!(d.is_word(hui));
        assert!(d.step(hui, Letter::from_char('s').unwrap()).is_some());
        assert!(d.walk(&word("xyz").unwrap()).is_none());
    }
}
