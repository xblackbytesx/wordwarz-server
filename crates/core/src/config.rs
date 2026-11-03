//! Game setup: the classic rules plus the modifiers a game may switch on
//! (decision 9). Fixed when the game is created; everything the core needs
//! to replay a game is this config, the seed and the action log.

use serde::{Deserialize, Serialize};

use crate::board::SIZES;
use crate::lexicon::Language;

/// Bumped whenever a rule changes how a stored game replays. A game keeps
/// the version it was created with; the core refuses versions it lacks.
pub const RULES_VERSION: u32 = 1;

pub const MIN_PLAYERS: usize = 2;
pub const MAX_PLAYERS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameConfig {
    pub rules_version: u32,
    pub language: Language,
    pub board: BoardSetup,
    /// One entry per seat, in turn order.
    pub players: Vec<PlayerSetup>,
    /// 2v2: seats 0 and 2 against 1 and 3; partners see each other's rack.
    #[serde(default)]
    pub teams: bool,
    pub bingo_bonus: u32,
    /// Hints per player (0 = off).
    #[serde(default)]
    pub hint_tokens: u8,
    /// Hidden bonus squares (0 = off).
    #[serde(default)]
    pub bounties: u8,
    #[serde(default)]
    pub comeback: Option<Comeback>,
    /// A swap needs at least this many tiles in the bag.
    pub swap_min_bag: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSetup {
    pub size: u8,
    /// Premiums drawn from the game seed instead of the fixed layout.
    #[serde(default)]
    pub random: bool,
}

/// Per-seat handicap (decision 10). Visible to everyone in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerSetup {
    pub rack_size: u8,
    /// Every move's score times this / 100, rounded half up.
    pub score_percent: u16,
}

impl Default for PlayerSetup {
    fn default() -> Self {
        PlayerSetup { rack_size: 7, score_percent: 100 }
    }
}

/// The trailing player's moves score extra while they are far behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comeback {
    /// Applies when the mover trails the leader by at least this many points.
    pub behind_by: u32,
    /// Extra percent of the move's score.
    pub percent: u16,
}

/// Starting points the new-game screen offers; every field stays tweakable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    #[serde(rename = "classic")]
    Classic,
    #[serde(rename = "family")]
    Family,
    #[serde(rename = "chaos")]
    Chaos,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("rules version {0} is not supported by this build")]
    RulesVersion(u32),
    #[error("a game has {MIN_PLAYERS} to {MAX_PLAYERS} players")]
    PlayerCount,
    #[error("team games need exactly four players")]
    TeamsNeedFour,
    #[error("board size must be one of 11, 15, 19")]
    BoardSize,
    #[error("rack size must be 5 to 9")]
    RackSize,
    #[error("score percent must be 50 to 200")]
    ScorePercent,
    #[error("at most 8 bounties and 9 hints")]
    TooMany,
    #[error("the bag is too small for these racks")]
    BagTooSmall,
}

impl GameConfig {
    pub fn classic(language: Language, players: usize) -> GameConfig {
        GameConfig {
            rules_version: RULES_VERSION,
            language,
            board: BoardSetup { size: 15, random: false },
            players: vec![PlayerSetup::default(); players],
            teams: false,
            bingo_bonus: 40,
            hint_tokens: 0,
            bounties: 0,
            comeback: None,
            swap_min_bag: 7,
        }
    }

    pub fn preset(preset: Preset, language: Language, players: usize) -> GameConfig {
        let mut c = GameConfig::classic(language, players);
        match preset {
            Preset::Classic => {}
            Preset::Family => {
                c.hint_tokens = 3;
                c.comeback = Some(Comeback { behind_by: 50, percent: 25 });
                c.teams = players == 4;
            }
            Preset::Chaos => {
                c.board.random = true;
                c.bounties = 4;
                c.comeback = Some(Comeback { behind_by: 40, percent: 50 });
            }
        }
        c
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.rules_version != RULES_VERSION {
            return Err(ConfigError::RulesVersion(self.rules_version));
        }
        if !(MIN_PLAYERS..=MAX_PLAYERS).contains(&self.players.len()) {
            return Err(ConfigError::PlayerCount);
        }
        if self.teams && self.players.len() != 4 {
            return Err(ConfigError::TeamsNeedFour);
        }
        if !SIZES.contains(&self.board.size) {
            return Err(ConfigError::BoardSize);
        }
        for p in &self.players {
            if !(5..=9).contains(&p.rack_size) {
                return Err(ConfigError::RackSize);
            }
            if !(50..=200).contains(&p.score_percent) {
                return Err(ConfigError::ScorePercent);
            }
        }
        if self.bounties > 8 || self.hint_tokens > 9 {
            return Err(ConfigError::TooMany);
        }
        // Racks must leave tiles to play with: at most half the bag on racks.
        let on_racks: u32 = self.players.iter().map(|p| p.rack_size as u32).sum();
        if on_racks * 2 > crate::tiles::LETTER_TILES {
            return Err(ConfigError::BagTooSmall);
        }
        Ok(())
    }

    /// The team of a seat (`None` outside team games).
    pub fn team_of(&self, seat: usize) -> Option<usize> {
        self.teams.then_some(seat % 2)
    }

    /// Whether `viewer` may see `seat`'s rack.
    pub fn sees_rack(&self, viewer: usize, seat: usize) -> bool {
        viewer == seat || (self.teams && viewer % 2 == seat % 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid() {
        for p in [Preset::Classic, Preset::Family, Preset::Chaos] {
            for n in 2..=4 {
                GameConfig::preset(p, Language::Dutch, n).validate().unwrap();
            }
        }
        assert!(GameConfig::preset(Preset::Family, Language::Dutch, 4).teams);
        assert!(!GameConfig::preset(Preset::Family, Language::Dutch, 3).teams);
    }

    #[test]
    fn validation_catches_bad_setups() {
        let mut c = GameConfig::classic(Language::English, 5);
        assert_eq!(c.validate(), Err(ConfigError::PlayerCount));
        c.players.pop();
        c.players[0].rack_size = 12;
        assert_eq!(c.validate(), Err(ConfigError::RackSize));
        c.players[0].rack_size = 8;
        c.teams = true;
        c.validate().unwrap();
        c.players.pop();
        assert_eq!(c.validate(), Err(ConfigError::TeamsNeedFour));
    }

    #[test]
    fn json_shape() {
        let c = GameConfig::classic(Language::Bilingual, 2);
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["language"], "nl+en");
        assert_eq!(j["board"]["size"], 15);
        assert_eq!(j["players"][0]["rackSize"], 7);
        assert_eq!(serde_json::from_value::<GameConfig>(j).unwrap(), c);
    }
}
