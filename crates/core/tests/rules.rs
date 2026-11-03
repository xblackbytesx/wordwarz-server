//! The rules against the real lexicons: scoring by hand, every rule error,
//! and whole bot-versus-bot games in every mode, replayed from their logs.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use wordwarz_core::board::{Dir, Pos};
use wordwarz_core::config::{Comeback, GameConfig, PlayerSetup, Preset};
use wordwarz_core::game::{Action, ActionError, EndReason, Game, Outcome, Status, TurnKind};
use wordwarz_core::letters::{Letter, PlacedTile, Tile};
use wordwarz_core::lexicon::{Dictionary, Language, Lexicon};
use wordwarz_core::moves::{self, MoveError, Placement};
use wordwarz_core::tilesets;

fn lexicon(file: &str) -> Arc<Lexicon> {
    let path = format!("{}/../../lexicons/{file}", env!("CARGO_MANIFEST_DIR"));
    Arc::new(Lexicon::from_bytes(std::fs::read(&path).expect("lexicons/*.fst (make lexicons)")).unwrap())
}

fn dict(language: Language) -> &'static Dictionary {
    static NL: OnceLock<Arc<Lexicon>> = OnceLock::new();
    static EN: OnceLock<Arc<Lexicon>> = OnceLock::new();
    static D: OnceLock<[Dictionary; 3]> = OnceLock::new();
    let d = D.get_or_init(|| {
        let nl = NL.get_or_init(|| lexicon("nl.fst")).clone();
        let en = EN.get_or_init(|| lexicon("en.fst")).clone();
        [Dictionary::single(nl.clone()), Dictionary::single(en.clone()), Dictionary::new(vec![nl, en])]
    });
    match language {
        Language::Dutch => &d[0],
        Language::English => &d[1],
        Language::Bilingual => &d[2],
    }
}

fn l(c: char) -> Letter {
    Letter::from_char(c).unwrap()
}

fn rack(s: &str) -> Vec<Tile> {
    s.chars().map(|c| Tile::from_char(c).unwrap()).collect()
}

/// Tiles spelling `word` from `(row, col)` in `dir`; lowercase letters are blanks.
fn lay(word: &str, row: u8, col: u8, dir: Dir) -> Vec<Placement> {
    word.chars()
        .enumerate()
        .map(|(i, c)| {
            let pos = match dir {
                Dir::Across => Pos::new(row, col + i as u8),
                Dir::Down => Pos::new(row + i as u8, col),
            };
            let tile = if c.is_lowercase() { PlacedTile::blank(l(c)) } else { PlacedTile::letter(l(c)) };
            Placement { pos, tile }
        })
        .collect()
}

fn play(g: &mut Game, seat: usize, tiles: Vec<Placement>) -> Result<TurnKind, ActionError> {
    let d = dict(g.config.language);
    match g.apply(seat, Action::Play { tiles }, Some(d))? {
        Outcome::Turn(t) => Ok(t.kind),
        Outcome::Hint(_) => unreachable!(),
    }
}

fn total_of(kind: &TurnKind) -> i32 {
    match kind {
        TurnKind::Play { score, .. } => score.total,
        _ => panic!("not a play: {kind:?}"),
    }
}

#[test]
fn lexicons_hold_the_expected_words() {
    let nl = dict(Language::Dutch);
    let en = dict(Language::English);
    for w in ["kat", "huis", "ideeen", "ij", "fiets", "gezellig"] {
        assert!(nl.contains_str(w), "nl {w}");
    }
    for w in ["cat", "quiz", "house", "xu"] {
        assert!(en.contains_str(w), "en {w}");
    }
    // Loans exist both ways (nl has "house", the music; en has "kat", a shrub).
    assert!(!nl.contains_str("throughout") && !en.contains_str("gezellig"));
    let both = dict(Language::Bilingual);
    assert!(both.contains_str("throughout") && both.contains_str("gezellig"));
    assert!(!nl.contains_str("kevin"), "proper nouns are out");
}

#[test]
fn tile_sets_have_102_tiles() {
    for lang in [Language::Dutch, Language::English, Language::Bilingual] {
        let t = tilesets::for_language(lang);
        assert_eq!(t.total(), 102, "{lang:?}");
        assert!(t.counts.iter().all(|&c| c >= 1));
        assert_eq!(t.value(l('e')), 1);
        assert_eq!(t.value(l('q')), 10);
    }
}

#[test]
fn scoring_by_hand() {
    let mut g = Game::new(GameConfig::classic(Language::Dutch, 2), 1).unwrap();
    let v = |c| tilesets::for_language(Language::Dutch).value(l(c));
    g.seats[0].rack = rack("KATSE?R");
    // KAT across the plain centre row: the bare letter values.
    let k = play(&mut g, 0, lay("KAT", 7, 6, Dir::Across)).unwrap();
    assert_eq!(total_of(&k), (v('k') + v('a') + v('t')) as i32);

    // Seat 1 hooks S under the T and builds down from (7,8): TAS, with
    // (8,8) and (9,8) plain; then down column 8.
    g.seats[1].rack = rack("ASEEEEE");
    let k = play(&mut g, 1, lay("AS", 8, 8, Dir::Down)).unwrap();
    assert_eq!(total_of(&k), (v('t') + v('a') + v('s')) as i32);

    // TAS becomes TASJE with a blank J on the double letter at (10,8):
    // the blank scores 0 there, the word still counts.
    g.seats[0].rack = rack("?EEEEEE");
    let k = play(&mut g, 0, lay("jE", 10, 8, Dir::Down)).unwrap();
    match &k {
        TurnKind::Play { words, .. } => assert_eq!(words[0].word, "TASJE"),
        _ => unreachable!(),
    }
    assert_eq!(total_of(&k), (v('t') + v('a') + v('s') + v('e')) as i32);
}

#[test]
fn premiums_count_only_under_new_tiles() {
    // Column 3 on the classic 15: (7,3) is a double word.
    let mut g = Game::new(GameConfig::classic(Language::English, 2), 1).unwrap();
    let v = |c| tilesets::for_language(Language::English).value(l(c));
    g.seats[0].rack = rack("HOUSEXZ");
    let k = play(&mut g, 0, lay("HOUSE", 7, 3, Dir::Across)).unwrap();
    let letters = v('h') + v('o') + v('u') + v('s') + v('e');
    assert_eq!(total_of(&k), letters as i32 * 2);
    // Extending HOUSE to HOUSES reuses (7,3) without its premium.
    g.seats[1].rack = rack("SAAAAAA");
    let k = play(&mut g, 1, lay("S", 7, 8, Dir::Across)).unwrap();
    assert_eq!(total_of(&k), (letters + v('s')) as i32);
}

#[test]
fn every_rule_error() {
    let mut g = Game::new(GameConfig::classic(Language::English, 2), 3).unwrap();
    g.seats[0].rack = rack("CATSDOG");
    let err = |g: &mut Game, t| play(g, 0, t).unwrap_err();
    assert_eq!(err(&mut g, vec![]), ActionError::Move(MoveError::Empty));
    assert_eq!(err(&mut g, lay("CAT", 0, 0, Dir::Across)), ActionError::Move(MoveError::MissesStart));
    assert_eq!(err(&mut g, lay("C", 7, 7, Dir::Across)), ActionError::Move(MoveError::TooShort));
    assert_eq!(err(&mut g, lay("CAT", 7, 13, Dir::Across)), ActionError::Move(MoveError::OffBoard));
    let mut gap = lay("CAT", 7, 6, Dir::Across);
    gap[2].pos.col += 1;
    assert_eq!(err(&mut g, gap), ActionError::Move(MoveError::Gap));
    let mut bent = lay("CAT", 7, 6, Dir::Across);
    bent[2].pos.row += 1;
    assert_eq!(err(&mut g, bent), ActionError::Move(MoveError::NotInLine));
    let mut dup = lay("CA", 7, 7, Dir::Across);
    dup[1].pos = dup[0].pos;
    assert_eq!(err(&mut g, dup), ActionError::Move(MoveError::Duplicate));
    assert_eq!(err(&mut g, lay("ZAP", 7, 6, Dir::Across)), ActionError::Move(MoveError::NotOnRack));
    assert_eq!(
        err(&mut g, lay("TCA", 7, 6, Dir::Across)),
        ActionError::Move(MoveError::NotWords { words: vec!["TCA".into()] })
    );
    assert_eq!(play(&mut g, 1, lay("CAT", 7, 6, Dir::Across)).unwrap_err(), ActionError::NotYourTurn);

    play(&mut g, 0, lay("CAT", 7, 6, Dir::Across)).unwrap();
    g.seats[1].rack = rack("DOGSAAA");
    assert_eq!(play(&mut g, 1, lay("DOG", 0, 0, Dir::Across)).unwrap_err(), ActionError::Move(MoveError::NotConnected));
    assert_eq!(play(&mut g, 1, lay("DOG", 7, 7, Dir::Down)).unwrap_err(), ActionError::Move(MoveError::Occupied));
}

#[test]
fn handicap_and_comeback() {
    let mut c = GameConfig::classic(Language::English, 2);
    c.players[1] = PlayerSetup { rack_size: 8, score_percent: 150 };
    c.comeback = Some(Comeback { behind_by: 5, percent: 50 });
    let mut g = Game::new(c, 5).unwrap();
    assert_eq!(g.seats[1].rack.len(), 8);
    g.seats[0].rack = rack("QUIZAAA");
    let first = total_of(&play(&mut g, 0, lay("QUIZ", 7, 7, Dir::Across)).unwrap());
    // Seat 1 trails by `first` and plays IT down from the I of QUIZ.
    g.seats[1].rack = rack("TSSSSSSS");
    let k = play(&mut g, 1, lay("T", 8, 9, Dir::Down)).unwrap();
    let TurnKind::Play { score, .. } = k else { unreachable!() };
    assert!(first >= 5);
    assert_eq!(score.handicap, ((score.base * 150 + 50) / 100 - score.base) as i32);
    assert_eq!(score.comeback, (score.base * 50 + 50) / 100);
    assert_eq!(score.total, score.base as i32 + score.handicap + score.comeback as i32);
}

#[test]
fn views_hide_what_they_should() {
    let mut g = Game::new(GameConfig::preset(Preset::Family, Language::Dutch, 4), 11).unwrap();
    assert!(g.config.teams);
    let v = g.view(Some(0));
    assert!(v.seats[0].rack.is_some() && v.seats[2].rack.is_some(), "own and partner rack");
    assert!(v.seats[1].rack.is_none() && v.seats[3].rack.is_none());
    let unseen: u32 = v.unseen.iter().map(|t| t.count).sum();
    assert_eq!(unseen as usize, g.bag.len() + 14);
    assert!(g.view(None).seats.iter().all(|s| s.rack.is_none()));
    let json = serde_json::to_string(&g.view(Some(1))).unwrap();
    assert!(!json.contains("\"bag\""), "no bag order in a view");

    // Hidden bounties never show before they are claimed.
    let mut c = GameConfig::classic(Language::Dutch, 2);
    c.bounties = 5;
    g = Game::new(c, 2).unwrap();
    assert_eq!(g.bounties.len(), 5);
    assert!(g.view(Some(0)).claimed_bounties.is_empty());
}

#[test]
fn resignations() {
    // Three players: one resigns, the others play on with the tiles back in the bag.
    let mut g = Game::new(GameConfig::classic(Language::English, 3), 8).unwrap();
    let before = g.bag.len();
    g.apply(1, Action::Resign, None).unwrap();
    assert_eq!(g.bag.len(), before + 7);
    assert!(!g.is_over());
    g.apply(0, Action::Pass, None).unwrap();
    assert_eq!(g.turn, 2, "the resigned seat is skipped");
    g.apply(2, Action::Resign, None).unwrap();
    assert!(matches!(&g.status, Status::Finished { reason: EndReason::Resigned, winners, .. } if winners == &vec![0]));

    // Teams: one partner resigning ends it for the team.
    let mut c = GameConfig::classic(Language::English, 4);
    c.teams = true;
    let mut g = Game::new(c, 8).unwrap();
    g.apply(3, Action::Resign, None).unwrap();
    assert!(matches!(&g.status, Status::Finished { winners, .. } if winners == &vec![0, 2]));
}

#[test]
fn hints_spend_tokens_and_find_a_move() {
    let mut c = GameConfig::classic(Language::English, 2);
    c.hint_tokens = 1;
    let mut g = Game::new(c, 4).unwrap();
    g.seats[0].rack = rack("RETAINS");
    let Outcome::Hint(Some(best)) = g.apply(0, Action::Hint, Some(dict(Language::English))).unwrap() else {
        panic!("no hint");
    };
    assert_eq!(best.scored.tiles_used, 7, "a seven-letter bingo exists for RETAINS");
    assert_eq!(g.apply(0, Action::Hint, None).unwrap_err(), ActionError::NoHints);
    assert_eq!(g.view(Some(0)).seats[0].hints_left, 0);
}

/// Plays a whole game with the best move every turn (swap or pass when
/// stuck), checking invariants after each action. Returns the game and its log.
fn bot_game(config: GameConfig, seed: u64) -> (Game, Vec<(usize, Action)>) {
    let d = dict(config.language);
    let total = tilesets::for_language(config.language).total() as usize;
    let mut g = Game::new(config, seed).unwrap();
    let mut log = Vec::new();
    for _ in 0..400 {
        if g.is_over() {
            break;
        }
        let seat = g.turn;
        let cands = g.candidates(seat, d, 3);
        for c in &cands {
            // Every generated move is legal by the full check.
            moves::evaluate(&g.board, &g.layout, g.tiles(), &c.tiles, g.config.bingo_bonus, Some(d)).unwrap();
        }
        let action = match cands.first() {
            Some(c) => Action::Play { tiles: c.tiles.clone() },
            None if g.bag.len() >= 7 => Action::Swap { tiles: g.seats[seat].rack.clone() },
            None => Action::Pass,
        };
        g.apply(seat, action.clone(), Some(d)).unwrap();
        log.push((seat, action));

        let on_board = g.board.cells.iter().flatten().count();
        let on_racks: usize = g.seats.iter().map(|s| s.rack.len()).sum();
        assert_eq!(on_board + on_racks + g.bag.len(), total, "tiles are conserved");
    }
    assert!(g.is_over(), "game did not finish");
    (g, log)
}

#[test]
fn bot_games_in_every_mode_replay_exactly() {
    let setups: Vec<GameConfig> = vec![
        GameConfig::classic(Language::Dutch, 2),
        GameConfig::classic(Language::English, 3),
        GameConfig::preset(Preset::Family, Language::Bilingual, 4),
        GameConfig::preset(Preset::Chaos, Language::Dutch, 2),
        {
            let mut c = GameConfig::preset(Preset::Chaos, Language::English, 4);
            c.board.size = 19;
            c.players[3].rack_size = 9;
            c
        },
        {
            let mut c = GameConfig::classic(Language::Dutch, 2);
            c.board.size = 11;
            c
        },
    ];
    for (i, config) in setups.into_iter().enumerate() {
        let started = Instant::now();
        let (g, log) = bot_game(config.clone(), 100 + i as u64);
        let plays = g.history.iter().filter(|t| matches!(t.kind, TurnKind::Play { .. })).count();
        let Status::Finished { reason, winners, .. } = &g.status else { unreachable!() };
        println!(
            "setup {i}: {} turns ({plays} plays) in {:?}, {reason:?}, winners {winners:?}, scores {:?}",
            g.history.len(),
            started.elapsed(),
            g.seats.iter().map(|s| s.score).collect::<Vec<_>>()
        );
        assert!(plays > 10, "setup {i} barely played");

        let replayed = Game::replay(config, 100 + i as u64, &log).unwrap();
        assert_eq!(replayed, g, "setup {i} replays to the same state");
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<Game>(&json).unwrap(), g);
    }
}

#[test]
fn the_view_previews_what_the_server_scores() {
    let config = GameConfig::preset(Preset::Family, Language::English, 4);
    let d = dict(Language::English);
    let mut g = Game::new(config, 21).unwrap();
    for _ in 0..12 {
        let seat = g.turn;
        let Some(best) = g.candidates(seat, d, 1).pop() else { break };
        let preview = g.view(Some(seat)).preview(&best.tiles, Some(d)).unwrap();
        let kind = play(&mut g, seat, best.tiles).unwrap();
        let TurnKind::Play { score, .. } = kind else { unreachable!() };
        assert_eq!(preview.score, score, "no bounties in this preset, so they match exactly");
    }
}

#[test]
fn rule_errors_have_a_stable_json_shape() {
    let e = ActionError::Move(MoveError::NotWords { words: vec!["TCA".into()] });
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({ "error": "move", "reason": "notWords", "words": ["TCA"] })
    );
    assert_eq!(serde_json::to_value(ActionError::NotYourTurn).unwrap(), serde_json::json!({ "error": "notYourTurn" }));
    let e = ActionError::Move(MoveError::Gap);
    assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"error":"move","reason":"gap"}"#);
}
