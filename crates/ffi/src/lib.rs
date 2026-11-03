//! wordwarz-ffi: the Android app's window into the rules engine.
//!
//! JSON in and out: views, actions, configs and errors cross as the core's
//! own JSON, the same shapes the server's `/api/v1` sends, so the app decodes
//! one set of models whether a game is online or on this phone. Refused moves
//! are expected outcomes, not exceptions: they come back as an `Outcome` with
//! `ok = false` and the core's error JSON.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};
use wordwarz_core::bot::{self, Level};
use wordwarz_core::config::{GameConfig, Preset, RULES_VERSION};
use wordwarz_core::game::{Action, Game, GameView, Outcome as CoreOutcome};
use wordwarz_core::lexicon::{Dictionary, Language, Lexicon};
use wordwarz_core::moves::Placement;
use wordwarz_core::{movegen, tilesets};

uniffi::setup_scaffolding!();

/// Flat: Kotlin gets the Display text as the exception message. Variant
/// fields named `message` would clash with `Throwable.message` in Kotlin.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum WwzError {
    /// Malformed JSON or an unknown name from the app: a bug, not a move.
    #[error("{message}")]
    Input { message: String },
}

fn input(e: impl std::fmt::Display) -> WwzError {
    WwzError::Input { message: e.to_string() }
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, json: &str) -> Result<T, WwzError> {
    serde_json::from_str(json).map_err(|e| input(format!("{what}: {e}")))
}

fn named<T: serde::de::DeserializeOwned>(what: &str, name: &str) -> Result<T, WwzError> {
    serde_json::from_value(Value::String(name.to_string())).map_err(|_| input(format!("unknown {what}: {name}")))
}

fn to_json(v: &impl Serialize) -> String {
    serde_json::to_string(v).expect("core types serialize")
}

/// The result of something the rules may refuse.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Outcome {
    pub ok: bool,
    /// On success the result's JSON; on refusal the core's error JSON
    /// (`{"error": ...}` or `{"reason": ...}`).
    pub json: String,
    /// Human-readable refusal (English), empty on success.
    pub message: String,
}

fn refused(err: &(impl Serialize + std::fmt::Display)) -> Outcome {
    Outcome { ok: false, json: to_json(err), message: err.to_string() }
}

/// Both word lists, loaded once from the app's assets.
#[derive(uniffi::Object)]
pub struct Lexicons {
    dutch: Dictionary,
    english: Dictionary,
    both: Dictionary,
}

impl Lexicons {
    fn get(&self, language: Language) -> &Dictionary {
        match language {
            Language::Dutch => &self.dutch,
            Language::English => &self.english,
            Language::Bilingual => &self.both,
        }
    }
}

#[uniffi::export]
impl Lexicons {
    /// `nl` and `en` are the bytes of `nl.fst` and `en.fst`.
    #[uniffi::constructor]
    pub fn new(nl: Vec<u8>, en: Vec<u8>) -> Result<Arc<Self>, WwzError> {
        let nl = Arc::new(Lexicon::from_bytes(nl).map_err(input)?);
        let en = Arc::new(Lexicon::from_bytes(en).map_err(input)?);
        Ok(Arc::new(Lexicons {
            dutch: Dictionary::single(nl.clone()),
            english: Dictionary::single(en.clone()),
            both: Dictionary::new(vec![nl, en]),
        }))
    }

    /// Whether `word` counts in `language` (`"nl"`, `"en"`, `"nl+en"`).
    pub fn contains(&self, language: String, word: String) -> Result<bool, WwzError> {
        Ok(self.get(named("language", &language)?).contains_str(&word))
    }
}

/// A game kept on this phone (pass-and-play, practice against the
/// computer). Holds the full state, bag included; the UI only ever renders
/// `view_json` for the seat whose turn it is.
#[derive(uniffi::Object)]
pub struct LocalGame {
    game: Mutex<Game>,
}

#[uniffi::export]
impl LocalGame {
    #[uniffi::constructor]
    pub fn create(config_json: String, seed: u64) -> Result<Arc<Self>, WwzError> {
        let config: GameConfig = parse("config", &config_json)?;
        let game = Game::new(config, seed).map_err(input)?;
        Ok(Arc::new(LocalGame { game: Mutex::new(game) }))
    }

    /// From `state_json` saved earlier.
    #[uniffi::constructor]
    pub fn restore(state_json: String) -> Result<Arc<Self>, WwzError> {
        let game: Game = parse("state", &state_json)?;
        game.config.validate().map_err(input)?;
        Ok(Arc::new(LocalGame { game: Mutex::new(game) }))
    }

    /// The whole state, to save.
    pub fn state_json(&self) -> String {
        to_json(&*self.game.lock().unwrap())
    }

    /// What `seat` sees (None: nobody's rack, as between turns).
    pub fn view_json(&self, seat: Option<u32>) -> String {
        to_json(&self.game.lock().unwrap().view(seat.map(|s| s as usize)))
    }

    pub fn turn(&self) -> u32 {
        self.game.lock().unwrap().turn as u32
    }

    pub fn is_over(&self) -> bool {
        self.game.lock().unwrap().is_over()
    }

    /// Applies a core `Action` (JSON). Success JSON: `{"turn": Turn}` or
    /// `{"hint": Candidate | null}`.
    pub fn apply(&self, seat: u32, action_json: String, lexicons: Arc<Lexicons>) -> Result<Outcome, WwzError> {
        let action: Action = parse("action", &action_json)?;
        let mut game = self.game.lock().unwrap();
        let dict = lexicons.get(game.config.language);
        Ok(match game.apply(seat as usize, action, Some(dict)) {
            Ok(CoreOutcome::Turn(t)) => {
                Outcome { ok: true, json: json!({ "turn": t }).to_string(), message: String::new() }
            }
            Ok(CoreOutcome::Hint(h)) => {
                Outcome { ok: true, json: json!({ "hint": h }).to_string(), message: String::new() }
            }
            Err(e) => refused(&e),
        })
    }

    /// The computer's action for `seat` at `level` (`"easy"`, `"medium"`,
    /// `"hard"`): a play, else a swap of the whole rack, else a pass.
    pub fn bot_action(&self, seat: u32, level: String, lexicons: Arc<Lexicons>, seed: u64) -> Result<String, WwzError> {
        let level: Level = named("level", &level)?;
        let game = self.game.lock().unwrap();
        let seat = seat as usize;
        let cands = game.candidates(seat, lexicons.get(game.config.language), bot::POOL);
        let action = match bot::choose(&cands, level, seed) {
            Some(c) => Action::Play { tiles: c.tiles.clone() },
            None if game.bag.len() >= game.config.swap_min_bag as usize => {
                Action::Swap { tiles: game.seats[seat].rack.clone() }
            }
            None => Action::Pass,
        };
        Ok(to_json(&action))
    }
}

/// Live score while dragging tiles, for any `GameView` (local or from the
/// server). Success JSON: the core's `Preview`; refusal: a `MoveError`.
/// Without lexicons the word check is skipped (geometry and score only).
#[uniffi::export]
pub fn preview(
    view_json: String,
    placements_json: String,
    lexicons: Option<Arc<Lexicons>>,
) -> Result<Outcome, WwzError> {
    let view: GameView = parse("view", &view_json)?;
    let placements: Vec<Placement> = parse("placements", &placements_json)?;
    let dict = lexicons.as_ref().map(|l| l.get(view.config.language));
    Ok(match view.preview(&placements, dict) {
        Ok(p) => Outcome { ok: true, json: to_json(&p), message: String::new() },
        Err(e) => refused(&e),
    })
}

/// Legal moves for the viewer's rack in `view_json`, best first (JSON array
/// of `Candidate`). Empty when the view shows no rack.
#[uniffi::export]
pub fn best_moves(view_json: String, lexicons: Arc<Lexicons>, limit: u32) -> Result<String, WwzError> {
    let view: GameView = parse("view", &view_json)?;
    let Some(rack) = view.viewer.and_then(|v| view.seats.get(v)).and_then(|s| s.rack.clone()) else {
        return Ok("[]".into());
    };
    let tiles = tilesets::for_language(view.config.language);
    let dict = lexicons.get(view.config.language);
    let c = movegen::best_moves(&view.board, &view.layout, tiles, &rack, dict, view.config.bingo_bonus, limit as usize);
    Ok(to_json(&c))
}

/// A preset's config (`"classic"`, `"family"`, `"chaos"`) as JSON, to tweak.
#[uniffi::export]
pub fn preset_config(preset: String, language: String, players: u32) -> Result<String, WwzError> {
    let preset: Preset = named("preset", &preset)?;
    let language: Language = named("language", &language)?;
    Ok(to_json(&GameConfig::preset(preset, language, players as usize)))
}

/// Why a config is invalid, or None when it is fine.
#[uniffi::export]
pub fn config_problem(config_json: String) -> Option<String> {
    match serde_json::from_str::<GameConfig>(&config_json) {
        Ok(c) => c.validate().err().map(|e| e.to_string()),
        Err(e) => Some(format!("config: {e}")),
    }
}

/// The tile set of a language: `{"counts": [26], "values": [26], "blanks": n}`.
#[uniffi::export]
pub fn tile_set(language: String) -> Result<String, WwzError> {
    let language: Language = named("language", &language)?;
    Ok(to_json(tilesets::for_language(language)))
}

#[uniffi::export]
pub fn rules_version() -> u32 {
    RULES_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexicons() -> Arc<Lexicons> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../lexicons");
        Lexicons::new(std::fs::read(format!("{dir}/nl.fst")).unwrap(), std::fs::read(format!("{dir}/en.fst")).unwrap())
            .unwrap()
    }

    #[test]
    fn a_local_game_against_the_computer_plays_to_the_end_and_restores() {
        let lex = lexicons();
        let config = preset_config("family".into(), "nl+en".into(), 2).unwrap();
        assert_eq!(config_problem(config.clone()), None);
        let g = LocalGame::create(config, 7).unwrap();
        for i in 0..300 {
            if g.is_over() {
                break;
            }
            let seat = g.turn();
            let level = if seat == 0 { "hard" } else { "easy" };
            let action = g.bot_action(seat, level.into(), lex.clone(), i).unwrap();
            let out = g.apply(seat, action, lex.clone()).unwrap();
            assert!(out.ok, "{}", out.message);
            // Halfway through, save and carry on from the copy.
            if i == 10 {
                let copy = LocalGame::restore(g.state_json()).unwrap();
                assert_eq!(copy.state_json(), g.state_json());
            }
        }
        assert!(g.is_over());
        let v: Value = serde_json::from_str(&g.view_json(None)).unwrap();
        assert_eq!(v["status"]["state"], "finished");
        assert!(v["seats"][0]["rack"].is_null(), "no rack without a viewer");
    }

    #[test]
    fn refusals_are_data_and_previews_score() {
        let lex = lexicons();
        let g = LocalGame::create(preset_config("classic".into(), "en".into(), 2).unwrap(), 1).unwrap();
        let out = g.apply(1, r#"{"type":"pass"}"#.into(), lex.clone()).unwrap();
        assert!(!out.ok);
        assert_eq!(out.json, r#"{"error":"notYourTurn"}"#);

        let view = g.view_json(Some(0));
        let best: Value = serde_json::from_str(&best_moves(view.clone(), lex.clone(), 1).unwrap()).unwrap();
        let tiles = best[0]["tiles"].to_string();
        let p = preview(view.clone(), tiles.clone(), Some(lex.clone())).unwrap();
        assert!(p.ok, "{}", p.message);
        let p: Value = serde_json::from_str(&p.json).unwrap();
        assert_eq!(p["score"]["base"], best[0]["scored"]["base"]);

        let off = preview(view, r#"[{"pos":{"row":0,"col":0},"tile":{"letter":"A"}}]"#.into(), None).unwrap();
        assert!(!off.ok);
        let e: Value = serde_json::from_str(&off.json).unwrap();
        assert!(e["reason"] == "missesStart" || e["reason"] == "notOnRack", "{e}");

        assert!(g.apply(0, "not json".into(), lex.clone()).is_err(), "bad JSON is a bug, not a move");
        assert!(lex.contains("nl".into(), "gezellig".into()).unwrap());
        assert!(lex.contains("xx".into(), "a".into()).is_err());
        let t: Value = serde_json::from_str(&tile_set("nl".into()).unwrap()).unwrap();
        assert_eq!(t["values"].as_array().unwrap().len(), 26);
    }
}
