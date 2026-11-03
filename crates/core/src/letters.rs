//! Letters and tiles. The game alphabet is A to Z for every language: Dutch
//! words with diacritics are folded on the way into the lexicon (decision 7)
//! and IJ is two tiles.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const ALPHABET: usize = 26;

/// One of A..Z, stored as 0..26. Serialized as the uppercase character.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Letter(u8);

impl Letter {
    pub fn from_index(i: usize) -> Option<Letter> {
        (i < ALPHABET).then_some(Letter(i as u8))
    }

    /// Accepts upper and lower case ASCII letters.
    pub fn from_char(c: char) -> Option<Letter> {
        let c = c.to_ascii_lowercase();
        c.is_ascii_lowercase().then(|| Letter(c as u8 - b'a'))
    }

    /// Every letter, A first.
    pub fn all() -> impl Iterator<Item = Letter> {
        (0..ALPHABET as u8).map(Letter)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// The lowercase ASCII byte the lexicons are keyed on.
    pub fn byte(self) -> u8 {
        b'a' + self.0
    }

    pub fn to_char(self) -> char {
        (b'A' + self.0) as char
    }
}

impl fmt::Debug for Letter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_char())
    }
}

impl fmt::Display for Letter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_char())
    }
}

impl Serialize for Letter {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_char(self.to_char())
    }
}

impl<'de> Deserialize<'de> for Letter {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let c = char::deserialize(d)?;
        Letter::from_char(c).ok_or_else(|| serde::de::Error::custom(format!("not a letter: {c:?}")))
    }
}

/// A tile on a rack or in the bag. Serialized as `"A"`..`"Z"`, or `"?"` for a blank.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Tile {
    Letter(Letter),
    Blank,
}

impl Tile {
    pub fn to_char(self) -> char {
        match self {
            Tile::Letter(l) => l.to_char(),
            Tile::Blank => '?',
        }
    }

    pub fn from_char(c: char) -> Option<Tile> {
        if c == '?' { Some(Tile::Blank) } else { Letter::from_char(c).map(Tile::Letter) }
    }
}

impl Serialize for Tile {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_char(self.to_char())
    }
}

impl<'de> Deserialize<'de> for Tile {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let c = char::deserialize(d)?;
        Tile::from_char(c).ok_or_else(|| serde::de::Error::custom(format!("not a tile: {c:?}")))
    }
}

/// A tile once it lies on the board: the letter it reads as, and whether it
/// is a blank (worth 0) standing in for that letter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct PlacedTile {
    pub letter: Letter,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blank: bool,
}

impl PlacedTile {
    pub fn letter(letter: Letter) -> Self {
        PlacedTile { letter, blank: false }
    }

    pub fn blank(letter: Letter) -> Self {
        PlacedTile { letter, blank: true }
    }

    /// The rack tile this placement consumes.
    pub fn rack_tile(self) -> Tile {
        if self.blank { Tile::Blank } else { Tile::Letter(self.letter) }
    }
}

/// Parses `"HELLO"` into letters; `None` on anything outside A..Z.
pub fn word(s: &str) -> Option<Vec<Letter>> {
    s.chars().map(Letter::from_char).collect()
}

/// Letters back to an uppercase string.
pub fn spell(letters: &[Letter]) -> String {
    letters.iter().map(|l| l.to_char()).collect()
}
